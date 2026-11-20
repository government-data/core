import { describe, expect, it } from "vitest";
import { diagnostics, shortCommit, versionLabel } from "./buildInfo";

describe("versionLabel", () => {
  it("shows one version when web and API match", () => {
    expect(versionLabel({ version: "1.0.3" }, { version: "1.0.3" })).toEqual({
      text: "v1.0.3",
      mismatch: false,
    });
  });

  it("does not flag a development build", () => {
    expect(versionLabel({ version: "dev" }, { version: "0.1.0" })).toEqual({
      text: "dev · API 0.1.0",
      mismatch: false,
    });
  });

  it("shows the commit of a development build", () => {
    expect(
      versionLabel({ version: "dev", commit: "" }, { version: "0.1.0", commit: "2b11a4c9f00d" }),
    ).toEqual({ text: "dev 2b11a4c · API 0.1.0", mismatch: false });
    expect(versionLabel({ version: "dev", commit: "ec258ca1" }, { version: "0.1.0" }).text).toBe(
      "dev ec258ca · API 0.1.0",
    );
  });

  it("shows both when they differ", () => {
    expect(versionLabel({ version: "1.0.3" }, { version: "1.0.2" })).toEqual({
      text: "web 1.0.3 · API 1.0.2",
      mismatch: true,
    });
  });
});

describe("diagnostics", () => {
  it("lists versions with short commits and where it runs", () => {
    const text = diagnostics(
      { version: "1.0.3", commit: "2b11a4c9f00d" },
      { version: "1.0.3", commit: null },
      {
        host: "admin.orquestador.app",
        userAgent: "UA",
        now: new Date("2026-10-18T23:00:00Z"),
        role: "admin",
      },
    );
    expect(text).toBe(
      [
        "Quality Data",
        "web: 1.0.3 (2b11a4c)",
        "api: 1.0.3",
        "host: admin.orquestador.app",
        "role: admin",
        "browser: UA",
        "time: 2026-10-18T23:00:00.000Z",
      ].join("\n"),
    );
    expect(shortCommit(undefined)).toBe("");
  });
});
