-- Relay usage and plans for the hosted Zwiim Cloud.
-- See docs/design/browser.md, section 15.
--
-- Every account is on a plan; new accounts start on Free. The Cloud counts
-- relay bytes per account, period and channel, and sends them here once a
-- minute through add_relay_usage(). Only the Cloud's service key reads or
-- writes these tables: row-level security is on with no policies, and
-- clients read their usage through the Cloud's GET /v1/usage.
--
-- Stored: account id, period, channel and byte counts. Never content, IP
-- addresses or URLs.

-- Plans. A null limit means unlimited. A paid plan later is a new row.
create table public.plans (
  id            text primary key,
  name          text not null,
  browser_bytes bigint check (browser_bytes >= 0),  -- browser channel, per period
  total_bytes   bigint check (total_bytes >= 0),    -- all channels, per period
  control_rate  bigint check (control_rate > 0),    -- control channel, bytes/s sustained
  created_at    timestamptz not null default now()
);

insert into public.plans (id, name, browser_bytes, total_bytes, control_rate)
values ('free', 'Free', 5368709120, 10737418240, 131072);  -- 5 GiB, 10 GiB, 128 KiB/s

-- Which plan each account is on, and its reset day.
create table public.account_plans (
  user_id       uuid primary key references auth.users (id) on delete cascade,
  plan_id       text not null default 'free' references public.plans (id),
  period_anchor timestamptz not null,  -- periods start on this day and time each month (UTC)
  updated_at    timestamptz not null default now()
);

-- Bytes relayed per account period and channel.
create table public.relay_usage (
  user_id      uuid not null references auth.users (id) on delete cascade,
  period_start timestamptz not null,
  channel      text not null check (channel in ('browser', 'control')),
  bytes        bigint not null default 0 check (bytes >= 0),
  updated_at   timestamptz not null default now(),
  primary key (user_id, period_start, channel)
);

alter table public.plans enable row level security;
alter table public.account_plans enable row level security;
alter table public.relay_usage enable row level security;
revoke all on public.plans, public.account_plans, public.relay_usage from anon, authenticated;

-- Every new account goes on Free, with its sign-up time as the reset day.
create function public.assign_free_plan() returns trigger
language plpgsql security definer set search_path = '' as $$
begin
  insert into public.account_plans (user_id, period_anchor)
  values (new.id, new.created_at)
  on conflict (user_id) do nothing;
  return new;
end
$$;

revoke execute on function public.assign_free_plan() from public, anon, authenticated;

create trigger on_auth_user_created
after insert on auth.users
for each row execute function public.assign_free_plan();

-- Existing accounts.
insert into public.account_plans (user_id, period_anchor)
select id, created_at from auth.users
on conflict (user_id) do nothing;

-- The period containing `at` for an account anchored at `anchor`. Periods
-- are one calendar month long and computed in UTC. Adding months to the
-- anchor (not to the previous period) clamps to the month's last day and
-- returns to the anchor day afterwards: an anchor on the 31st gives
-- Jan 31, Feb 28, Mar 31.
create function public.relay_period(anchor timestamptz, at timestamptz,
                                    out period_start timestamptz, out resets_at timestamptz)
language plpgsql immutable set search_path = '' as $$
declare
  a timestamp := anchor at time zone 'UTC';
  t timestamp := at at time zone 'UTC';
  k int := (extract(year from t)::int - extract(year from a)::int) * 12
         + (extract(month from t)::int - extract(month from a)::int);
begin
  if a + make_interval(months => k) > t then
    k := k - 1;
  end if;
  period_start := (a + make_interval(months => k)) at time zone 'UTC';
  resets_at := (a + make_interval(months => k + 1)) at time zone 'UTC';
end
$$;

-- Adds a batch of byte counts and returns, for every account in the batch,
-- its plan, this period's usage and its limits.
--
-- `batch` is a JSON array of {"user_id": uuid, "channel": "browser" |
-- "control", "bytes": n}. A zero-byte entry only reads the account's
-- totals. Entries for unknown or non-uuid user ids are skipped (the Cloud
-- also accepts non-Supabase tokens); those accounts get no row back and the
-- Cloud treats them as unlimited. An account with no account_plans row is
-- treated as Free, anchored at its sign-up time.
create function public.add_relay_usage(batch jsonb)
returns table (
  user_id       uuid,
  plan_id       text,
  plan_name     text,
  period_start  timestamptz,
  resets_at     timestamptz,
  browser_used  bigint,
  control_used  bigint,
  browser_limit bigint,
  total_limit   bigint,
  control_rate  bigint
)
language plpgsql security definer set search_path = '' as $$
begin
  if jsonb_typeof(batch) is distinct from 'array' then
    raise exception 'add_relay_usage: batch must be a JSON array';
  end if;
  if exists (
    select 1 from jsonb_to_recordset(batch) as e(user_id text, channel text, bytes bigint)
    where e.channel is null or e.channel not in ('browser', 'control')
       or e.bytes is null or e.bytes < 0
  ) then
    raise exception 'add_relay_usage: every entry needs channel browser or control and bytes >= 0';
  end if;

  create temporary table pg_temp.relay_batch on commit drop as
  select u.id as user_id,
         coalesce(ap.plan_id, 'free') as plan_id,
         p.period_start,
         p.resets_at,
         e.channel,
         sum(e.bytes)::bigint as bytes
  from jsonb_to_recordset(batch) as e(user_id text, channel text, bytes bigint)
  join auth.users u
    on e.user_id ~* '^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$'
   and u.id = e.user_id::uuid
  left join public.account_plans ap on ap.user_id = u.id
  cross join lateral public.relay_period(coalesce(ap.period_anchor, u.created_at), now()) p
  group by u.id, ap.plan_id, p.period_start, p.resets_at, e.channel;

  insert into public.relay_usage as r (user_id, period_start, channel, bytes)
  select b.user_id, b.period_start, b.channel, b.bytes
  from pg_temp.relay_batch b
  where b.bytes > 0
  on conflict on constraint relay_usage_pkey
  do update set bytes = r.bytes + excluded.bytes, updated_at = now();

  return query
  select a.user_id, a.plan_id, pl.name, a.period_start, a.resets_at,
         coalesce(sum(r.bytes) filter (where r.channel = 'browser'), 0)::bigint,
         coalesce(sum(r.bytes) filter (where r.channel = 'control'), 0)::bigint,
         pl.browser_bytes, pl.total_bytes, pl.control_rate
  from (select distinct b.user_id, b.plan_id, b.period_start, b.resets_at
        from pg_temp.relay_batch b) a
  join public.plans pl on pl.id = a.plan_id
  left join public.relay_usage r
    on r.user_id = a.user_id and r.period_start = a.period_start
  group by a.user_id, a.plan_id, pl.name, a.period_start, a.resets_at,
           pl.browser_bytes, pl.total_bytes, pl.control_rate;

  drop table pg_temp.relay_batch;
end
$$;

revoke execute on function public.add_relay_usage(jsonb) from public, anon, authenticated;
revoke execute on function public.relay_period(timestamptz, timestamptz) from public, anon, authenticated;
grant execute on function public.add_relay_usage(jsonb) to service_role;
