export function errMessage(e: unknown): string {
  if (typeof e === "string") return e;
  if (e instanceof Error) return e.message;
  return JSON.stringify(e);
}

/**
 * A failure as the user reads it: a sentence, capital first and a full stop
 * last (`studio/design/behaviour.md`). Errors from git, Jira or Slack arrive as
 * fragments ("invalid_auth", "push rejected").
 */
export function failureText(e: unknown): string {
  const text = errMessage(e).trim();
  if (!text) return "Something went wrong. Try again.";
  const capital = text[0].toUpperCase() + text.slice(1);
  return /[.!?…:)]$/.test(capital) ? capital : `${capital}.`;
}
