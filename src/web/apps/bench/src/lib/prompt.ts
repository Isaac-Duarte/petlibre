// A job's question in words and buttons. The server holds the question
// (its words come from the installer) and checks every answer against its
// number and kind, so the page only offers the answers a question takes
// and says why one was refused.

import type { Answer, AnswerError, Expects, Prompt } from "@wellfed/api";

export interface Choice {
  answer: Answer;
  label: string;
}

/** The buttons a question takes: done for an instruction, yes or no for a question. */
export function choices(expects: Expects): Choice[] {
  if (expects === "done") return [{ answer: "done", label: "Done" }];
  return [
    { answer: "yes", label: "Yes" },
    { answer: "no", label: "No" },
  ];
}

/** A setup code's presses, in order, to show one per button; empty for any other question. */
export function codePresses(prompt: Prompt): readonly string[] {
  return prompt.kind === "key-setup-code" ? prompt.presses : [];
}

/** Why the server did not take an answer; `null` when it said nothing readable. */
export function refusalText(refusal: AnswerError | null, status?: number): string {
  if (status === 403) return "this link's token is not the running server's; open the link the console printed";
  if (!refusal) return "the bench server did not take the answer";
  switch (refusal.refused) {
    case "not-running":
      return `job ${refusal.job} is not running`;
    case "no-prompt":
      return `job ${refusal.job} asks nothing now`;
    case "wrong-seq":
      return `job ${refusal.job} asks question ${refusal.pending}, not ${refusal.seq}`;
    case "mismatched":
      return `the ${refusal.name} prompt takes ${refusal.expects === "done" ? "done" : "yes or no"}, not ${refusal.answer}`;
  }
}
