import { describe, expect, it } from "vitest";
import { choices, refusalText } from "./prompt";

describe("prompt", () => {
  it("offers done for an instruction and yes or no for a question", () => {
    expect(choices("done").map((c) => c.answer)).toEqual(["done"]);
    expect(choices("yes-no").map((c) => c.answer)).toEqual(["yes", "no"]);
  });

  it("says why an answer was refused", () => {
    expect(refusalText({ refused: "wrong-seq", job: 4, seq: 1, pending: 2 })).toBe("job 4 asks question 2, not 1");
    expect(refusalText({ refused: "mismatched", name: "recognize-feeder", expects: "yes-no", answer: "done" })).toBe(
      "the recognize-feeder prompt takes yes or no, not done",
    );
    expect(refusalText({ refused: "not-running", job: 3 })).toBe("job 3 is not running");
    expect(refusalText(null)).toBe("the bench server did not take the answer");
    expect(refusalText(null, 403)).toContain("open the link the console printed");
  });
});
