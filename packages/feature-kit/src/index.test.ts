import { describe, expect, it, vi } from "vitest";
import {
  DEFAULT_NAV_ORDER,
  defineFeature,
  isRoleAllowed,
  onSignOut,
  sessionExpired,
  signedOut,
  type FeatureModule,
} from "./index";

describe("defineFeature", () => {
  it("returns the module unchanged (identity)", () => {
    const feature: FeatureModule = {
      id: "sample",
      title: "Sample",
      routes: [{ path: "/sample", component: () => null }],
    };
    expect(defineFeature(feature)).toBe(feature);
  });
});

describe("DEFAULT_NAV_ORDER", () => {
  it("is a stable numeric fallback weight", () => {
    expect(DEFAULT_NAV_ORDER).toBe(100);
  });
});

describe("isRoleAllowed", () => {
  it("returns true when allowedRoles is undefined", () => {
    expect(isRoleAllowed("viewer")).toBe(true);
  });

  it("returns true when allowedRoles is empty", () => {
    expect(isRoleAllowed("viewer", [])).toBe(true);
  });

  it("returns true when userRole is in allowedRoles", () => {
    expect(isRoleAllowed("admin", ["admin", "owner"])).toBe(true);
  });

  it("returns false when userRole is not in allowedRoles", () => {
    expect(isRoleAllowed("viewer", ["admin"])).toBe(false);
  });
});

describe("the session ending", () => {
  it("has every feature forget what it kept, signing out or expired", () => {
    const forget = vi.fn();
    onSignOut(forget);
    signedOut();
    expect(forget).toHaveBeenCalledTimes(1);
    vi.stubGlobal("window", { location: { pathname: "/login", search: "" } });
    sessionExpired();
    vi.unstubAllGlobals();
    expect(forget).toHaveBeenCalledTimes(2);
  });
});
