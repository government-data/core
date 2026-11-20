import { strToU8 } from "fflate";
import { describe, expect, it } from "vitest";
import { changing, readUploaded, zipRules } from "./rulesArchive";

const id = { file: "id.yaml", yaml: "dataset: loans\nrules: []\n" };
const amount = { file: "amount.yaml", yaml: "dataset: loans\nrules: []\n" };

describe("rules in and out", () => {
  it("reads back from its .zip what was exported, other files named as ignored", () => {
    const zip = zipRules([id, amount]);
    const { files, ignored } = readUploaded([
      { name: "loans.zip", bytes: zip },
      { name: "notes.txt", bytes: strToU8("x") },
      { name: "Bad Name.yaml", bytes: strToU8("x") },
    ]);
    expect(files.sort((a, b) => a.file.localeCompare(b.file))).toEqual([amount, id]);
    expect(ignored).toEqual(["notes.txt", "Bad Name.yaml"]);
  });

  it("deploys only what would change main, against main's text or none", () => {
    const changed = { ...id, yaml: id.yaml.replace("[]", "[ ]") };
    const fresh = { file: "new.yaml", yaml: "dataset: loans\nrules: []\n" };
    expect(changing([amount, changed, fresh], [id, amount])).toEqual([
      { ...changed, expected: id.yaml },
      { ...fresh, expected: null },
    ]);
  });
});
