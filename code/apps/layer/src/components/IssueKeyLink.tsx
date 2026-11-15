import type { MouseEvent } from "react";
import { goTo } from "../lib/goto";
import { keyProjects } from "../lib/issue-keys";
import { useStore } from "../store";

/**
 * The projects whose keys are links (ACP-18): a task's ticket, a ticket read,
 * the project set in Settings. One string, so it changes only when they do.
 */
export function useKeyProjects(): string {
  return useStore((s) =>
    keyProjects([
      ...s.tasks.map((t) => t.issue_key),
      ...s.issues.map((i) => i.key),
      s.settings?.jira?.project_key ? `${s.settings.jira.project_key}-1` : null,
    ]),
  );
}

/** A ticket's key: its task in Work, else the ticket in Tickets. */
export function IssueKeyLink({ issueKey }: { issueKey: string }) {
  const task = useStore((s) => s.tasks.find((t) => t.issue_key === issueKey)?.id ?? null);
  function open(e: MouseEvent) {
    e.preventDefault();
    e.stopPropagation();
    goTo(task ? `task:${task}` : `ticket:${issueKey}`);
  }
  return (
    <a
      className="ui-link"
      href={`#${issueKey}`}
      title={task ? `Open ${issueKey}'s task in Work` : `Show ${issueKey} in Tickets`}
      onClick={open}
    >
      {issueKey}
    </a>
  );
}
