import type { Answer, Dialog } from "../../bridge";

export interface Choice {
  label: string;
  answer: Answer;
}

/** What a dialog offers: an approval is Approve or Deny; a question, its own options. */
export function choices(dialog: Dialog): Choice[] {
  switch (dialog.kind) {
    case "Confirm":
      return [
        { label: "Approve", answer: { Confirm: true } },
        { label: "Deny", answer: { Confirm: false } },
      ];
    case "Select":
      return dialog.options.map((option) => ({ label: option, answer: { Value: option } }));
    case "Input":
      return [];
  }
}
