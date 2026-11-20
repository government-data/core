import { describe, expect, it } from "vitest";
import { afterSignOut } from "./signOut";

describe("afterSignOut", () => {
  it("sends hosted installations back to the product home", () => {
    expect(afterSignOut("admin.orquestador.app")).toBe("https://orquestador.app/");
  });

  it("keeps a customer's own installation on its login page", () => {
    expect(afterSignOut("orquestador.empresa.local")).toBe("/login");
    expect(afterSignOut("localhost")).toBe("/login");
    expect(afterSignOut("evilorquestador.app")).toBe("/login");
  });
});
