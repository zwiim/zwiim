# Zwiim Privacy Policy

Last updated: [DATE]

This policy explains what data Zwiim handles, where it goes, and what you can
do about it. Zwiim is run by Ashish Kumar Shakya ("we", "us"). Contact:
hello@zwiim.com.

## The short version

1. Zwiim runs on your own computer. Your code, files, terminals, agent chats
   and Git data stay on that computer. We do not receive them.
2. A Zwiim account is optional. You need one only to reach your computer from
   another device, such as your phone.
3. With an account, we keep your account id and email, and the public keys and
   name of each computer you connect. Remote traffic passes through our relay
   end-to-end encrypted. The relay cannot read it.
4. We do not sell your data or show you ads.
5. Zwiim does not collect crash reports or usage analytics today. If we add
   them, we will update this policy first and tell you in the app.

## 1. Data that stays on your computer

The Zwiim Host and Desktop app keep their data on your computer: workspaces,
sessions, agent conversations, terminal output, file contents, Git data,
settings and the list of devices you paired. Credentials (your Zwiim session,
agent API keys, Git tokens) are kept in your operating system's keychain.

We do not receive this data. It never leaves your computer unless you send it
somewhere yourself, for example:

1. **Coding agents.** When you run an agent such as Claude Code, Codex or
   Gemini CLI, that agent sends your prompts and the code it reads to its own
   provider. That happens under the agent provider's terms and privacy policy,
   not ours. Zwiim does not copy that traffic to us.
2. **Git hosts.** Pushing, pulling or opening pull requests talks to your Git
   host (for example GitHub) under its terms.
3. **Paired devices.** A device you paired can see and control what the Host
   can, because that is what pairing is for.

## 2. Zwiim account (optional)

Signing in uses "Continue with Google" through Supabase, our authentication
provider.

| Data | Why | Where |
|---|---|---|
| Account id, email address, and the basic profile Google shares (such as your name) | Sign you in, show which account is in use | Supabase |
| Sign-in session | Keep you signed in | Your device's keychain, and Supabase |

We keep account data until you delete your account. Supabase stores it in
Mumbai, India (AWS region ap-south-1).

We do not get your Google password, and we do not read your Gmail, contacts or
other Google data.

## 3. Zwiim Cloud (remote access)

Zwiim Cloud connects your devices to your computer when they are not on the
same network. It holds:

| Data | Why | Kept |
|---|---|---|
| For each connected computer: a host id, your account id, the name you gave it, its two public keys, and when it was connected | Show your computers and check that a connection really comes from them | Until you ask us to remove the computer or delete your account |
| Online/offline state of your computers | Show presence | In memory only |
| Connection logs: host id, connection ids, times, and why a connection opened or closed | Run, debug and protect the service | 30 days |
| IP addresses of connections | Rate limiting and abuse protection | In memory, for a few minutes; Zwiim Cloud does not write them to its logs |

**Relay traffic is end-to-end encrypted** between your device and your
computer (Noise protocol, keys pinned at pairing). The relay forwards bytes it
cannot decrypt. It does not see your code, files, terminal output or agent
chats.

The Cloud runs on DigitalOcean servers in Bangalore, India, behind Cloudflare.
Cloudflare handles the outer connection, so it sees connection details such as
IP addresses and times, but not the encrypted content inside.

## 4. Updates and downloads

The Desktop app checks GitHub Releases for updates, and installing an agent
downloads it from its publisher (for example npm). Those services see your IP
address and the request, under their own terms.

## 5. Crash reports and analytics (not active yet)

Zwiim does not send crash reports or usage analytics today.

We plan to add:

1. **Crash and error reporting** with Sentry, to find and fix bugs. A report
   can include the app version, operating system, the error and a stack trace.
   We will strip file contents, prompts, terminal output, tokens and file paths
   that name you before a report leaves your device.
2. **Product analytics**, to learn which features people use. These would be
   counts of events (for example "a session started"), not your content.

Before turning either on, we will update this policy, name each provider, and
tell you in the app. Both will be on by default, and you can turn each off in
Settings at any time. Where the law requires your consent first, for example
in the European Union, we will ask before turning analytics on.

## 6. Who we share data with

We share data only with the providers that run the service for us:

| Provider | Role |
|---|---|
| Supabase | Account sign-in |
| Google | Sign-in identity provider |
| DigitalOcean | Hosts Zwiim Cloud |
| Cloudflare | Network edge in front of Zwiim Cloud |
| Sentry (planned) | Crash and error reports |

We do not sell personal data. We may disclose data if the law requires it,
and will tell you when we are allowed to.

## 7. Your choices and rights

1. **Use Zwiim without an account.** Everything local works signed out.
2. **Remove a computer** from your account by emailing hello@zwiim.com.
3. **Delete your account** by emailing hello@zwiim.com from the account's
   email address. We delete your account and Cloud records within 30 days.
4. **Ask for a copy** of the data we hold about you, or to correct it, at the
   same address.
5. **Withdraw consent.** Sign out, or turn off crash reports and analytics in
   Settings once they exist. Withdrawing does not affect what happened before.
6. **Nominate someone** to use these rights for you if you die or cannot act,
   by telling us at the same address.

We answer requests within 30 days. Our grievance officer is Ashish Kumar
Shakya, at hello@zwiim.com. If you are not satisfied, you can complain
to the Data Protection Board of India, or to your local data protection
authority.

Depending on where you live (for example under the GDPR in Europe or the DPDP
Act in India), you may have more rights.

**Why we may use your data (GDPR).** We use account and Cloud data to provide
the Service you asked for (contract), and connection logs and crash reports to
keep it working and secure (legitimate interests). Product analytics will rely
on your consent where the law requires it.

**Transfers.** Our servers are in India, and our providers may process data in
other countries, such as the United States. Where needed, we rely on the
providers' standard contractual clauses.

## 8. Children

A Zwiim account is for people aged 18 or over, and we do not knowingly
collect data from anyone younger. The software works without an account, and
local use sends us nothing. If you believe someone under 18 has an account,
email hello@zwiim.com and we will delete it.

## 9. Security

Credentials are kept in your operating system's keychain. Devices pair with
one-time codes and keys, and you can revoke any device. Remote traffic is
end-to-end encrypted. See `SECURITY.md` to report a vulnerability.

## 10. Changes

We will post changes here and update the date at the top. For material changes,
such as turning on analytics, we will also tell you in the app.

## 11. Self-hosting

If you run your own Zwiim Cloud, you are the operator of that service and this
policy does not cover it.
