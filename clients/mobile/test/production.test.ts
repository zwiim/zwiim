import { describe, expect, it } from "vitest";
import { productionProblems } from "../scripts/check-production.mjs";
import appJson from "../app.json";

const prod = {
  EXPO_PUBLIC_ZWIIM_CLOUD_URL: "https://cloud.zwiim.dev",
  EXPO_PUBLIC_ZWIIM_SUPABASE_URL: "https://prodref.supabase.co",
  EXPO_PUBLIC_ZWIIM_SUPABASE_ANON_KEY: "anon",
};

describe("a production build's configuration", () => {
  it("accepts production services", () => {
    expect(productionProblems(prod, appJson)).toEqual([]);
  });

  it("refuses development services and provisioning", () => {
    const problems: string[] = productionProblems(
      {
        ...prod,
        EXPO_PUBLIC_ZWIIM_CLOUD_URL: "https://abc.trycloudflare.com",
        EXPO_PUBLIC_ZWIIM_SUPABASE_URL: "https://devref.supabase.co",
        ZWIIM_DEV_SUPABASE_URL: "https://devref.supabase.co",
        EXPO_PUBLIC_ZWIIM_DEV_LINK: "zwiim://dev?provision=x",
      },
      appJson,
    );
    expect(problems).toEqual([
      "EXPO_PUBLIC_ZWIIM_CLOUD_URL is a temporary tunnel",
      "EXPO_PUBLIC_ZWIIM_SUPABASE_URL is the development project",
      "EXPO_PUBLIC_ZWIIM_DEV_LINK is set: development provisioning in a store build",
    ]);
    expect(productionProblems({ ...prod, EXPO_PUBLIC_ZWIIM_CLOUD_URL: "http://127.0.0.1:8787" }, appJson)).toEqual([
      "EXPO_PUBLIC_ZWIIM_CLOUD_URL must be https",
      "EXPO_PUBLIC_ZWIIM_CLOUD_URL points at this machine",
    ]);
    expect(productionProblems({}, appJson)).toHaveLength(3);
  });
});
