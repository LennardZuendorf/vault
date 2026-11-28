import { describe, expect, spyOn, test } from "bun:test";
import { api } from "../src/lib/api";
import { readsTicket } from "../src/lib/tracker";
import type { CheckoutPr, Settings, TaskPrs, TaskView, Ticket } from "../src/lib/types";
import { useStore } from "../src/store";

// The backend, as far as these refreshes ask it.
const backend = api as unknown as Record<string, (...args: unknown[]) => Promise<unknown>>;

describe("a refresh that found nothing new", () => {
  test("keeps the settings object every settings view holds", async () => {
    const settings = {
      jira_connected: false,
      github_connected: false,
      github: null,
    } as unknown as Settings;
    backend.getSettings = async () => structuredClone(settings);
    await useStore.getState().refreshSettings();
    const held = useStore.getState().settings;
    await useStore.getState().refreshSettings();
    expect(useStore.getState().settings).toBe(held);
  });

  test("after Sync or Clean up, keeps the projects and agents it already had", async () => {
    const projects = [{ id: "p", name: "web", path: "/tmp/web", default_branch: "main" }];
    const agents = [{ id: "claude", name: "Claude Code", program: "claude", installed: true }];
    backend.listProjects = async () => structuredClone(projects);
    backend.listTasks = async () => [];
    backend.listPanes = async () => [];
    backend.listAgents = async () => structuredClone(agents);
    backend.cursorIdeInstalled = async () => false;
    await useStore.getState().refreshAll();
    const before = useStore.getState();
    await useStore.getState().refreshAll();
    const after = useStore.getState();
    expect(after.projects).toBe(before.projects);
    expect(after.agents).toBe(before.agents);
    expect(after.tasks).toBe(before.tasks);
    expect(after.panes).toBe(before.panes);
  });
});

test("older direct and sweep PR replies cannot overwrite a newer task refresh", async () => {
  const initial = useStore.getState();
  const taskId = "ordered-pr-task";
  const older = Promise.withResolvers<CheckoutPr[]>();
  const sweep = Promise.withResolvers<TaskPrs[]>();
  const row: CheckoutPr = {
    checkout_id: "checkout",
    repo: "web",
    pin: null,
    pin_revision: "0",
    pin_read: false,
    action_target: null,
    pr: null,
    past: [],
    checks: [],
    reviews: [],
    verdict: "none",
    base: "new",
    changed: 0,
    to_reply: 0,
    error: null,
    branch_covered: true,
    detached_unlanded: false,
    covered_merge: false,
  };
  const direct = spyOn(api, "githubTaskPrs")
    .mockImplementationOnce(() => older.promise)
    .mockResolvedValue([row]);
  const all = spyOn(api, "githubAllPrs").mockImplementation(() => sweep.promise);
  useStore.setState({
    tasks: [{ id: taskId } as TaskView],
    settings: { github_connected: true } as Settings,
  });
  const first = useStore.getState().refreshTaskPrs(taskId);
  const poll = useStore.getState().refreshPrs();
  try {
    await useStore.getState().refreshTaskPrs(taskId);
    older.resolve([{ ...row, base: "old direct" }]);
    sweep.resolve([{ task_id: taskId, rows: [{ ...row, base: "old sweep" }], ticket: null }]);
    await Promise.all([first, poll]);
    expect(useStore.getState().prs[taskId]).toEqual([row]);
  } finally {
    direct.mockRestore();
    all.mockRestore();
    useStore.setState({ tasks: initial.tasks, settings: initial.settings, prs: initial.prs });
  }
});

test("switching GitHub endpoints immediately revokes old pin links and rejects in-flight replies", async () => {
  const current = useStore.getState();
  const taskId = "connection-changed-task";
  const github = {
    api_url: "https://api.old.example",
    web_url: "https://old.example",
    review_team: null,
  };
  const before = { ...current.settings, github, github_connected: true } as Settings;
  const next = { ...before, github: { ...github, web_url: "https://new.example" } };
  const pin = { repo: "acme/app", number: 42, url: "https://old.example/acme/app/pull/42" };
  const row: CheckoutPr = {
    checkout_id: "c",
    repo: "app",
    pin,
    pin_revision: "1",
    pin_read: true,
    pr: {
      ...pin,
      title: "Work",
      state: "open",
      draft: false,
      author: "dev",
      head: "topic",
      head_sha: "123",
      base: "main",
      mergeable_state: null,
      merged: false,
      comments: 0,
      review_comments: 0,
    },
    past: [],
    checks: [],
    reviews: [],
    verdict: "none",
    base: "main",
    changed: 1,
    to_reply: 0,
    error: null,
    branch_covered: false,
    detached_unlanded: false,
    covered_merge: false,
    action_target: "old-target",
  };
  const oldDirect = Promise.withResolvers<CheckoutPr[]>();
  const oldSweep = Promise.withResolvers<TaskPrs[]>();
  const direct = spyOn(api, "githubTaskPrs").mockReturnValue(oldDirect.promise);
  const sweep = spyOn(api, "githubAllPrs").mockReturnValue(oldSweep.promise);
  const readSettings = spyOn(api, "getSettings").mockResolvedValue(next);
  const queue = spyOn(api, "githubReviewQueue").mockRejectedValue("Fixture");
  useStore.setState({
    settings: before,
    tasks: [{ id: taskId } as TaskView],
    prs: { [taskId]: [row] },
  });
  const first = useStore.getState().refreshTaskPrs(taskId);
  const second = useStore.getState().refreshPrs();
  try {
    await useStore.getState().refreshSettings();
    expect(useStore.getState().prs[taskId][0]).toMatchObject({
      pin: { ...pin, url: "" },
      pin_read: false,
      pr: null,
      error: expect.any(String),
    });
    const revoked = useStore.getState().prs[taskId];
    oldDirect.resolve([row]);
    oldSweep.resolve([{ task_id: taskId, rows: [row], ticket: null }]);
    await Promise.all([first, second]);
    expect(useStore.getState().prs[taskId]).toBe(revoked);
  } finally {
    oldDirect.resolve([]);
    oldSweep.resolve([]);
    await Promise.allSettled([first, second]);
    for (const fixture of [direct, sweep, readSettings, queue]) fixture.mockRestore();
    const { settings, tasks, prs, reviewQueue, reviewQueueError } = current;
    useStore.setState({ settings, tasks, prs, reviewQueue, reviewQueueError });
  }
});

test("a linked ticket queues a new read behind the old poll without changing Mine", async () => {
  const saved: TaskView = {
    id: "local-task",
    name: "Fix login",
    root: "/tmp/local-task",
    branch: "fix-login",
    issue_key: "ACME-42",
    issue_url: "https://acme.example/browse/ACME-42",
    created_at: "2026-11-01T12:00:00Z",
    checkouts: [],
    pane_count: 0,
  };
  const { tasks, selectedTask, linkedTickets, issues } = useStore.getState();
  const older = Promise.withResolvers<TaskView[]>();
  const load = spyOn(api, "listTasks")
    .mockImplementationOnce(() => older.promise)
    .mockResolvedValue([saved]);
  const deleted = { ...saved, id: "deleted-task" };
  useStore.setState({ tasks: [deleted], selectedTask: null });
  const poll = useStore.getState().refreshTasks({ poll: true });
  const nudge = useStore.getState().taskTicketLinked({
    task_id: saved.id,
    key: "ACME-42",
    url: "https://acme.example/browse/ACME-42",
    status: "Closed",
    status_category: "done",
    context_warning: "Linked, but the agent context could not be refreshed.",
  });
  try {
    expect(load).toHaveBeenCalledTimes(1);
    older.resolve([]);
    await Promise.all([poll, nudge]);

    const now = useStore.getState();
    expect(now.tasks).toEqual([saved]);
    expect(now.issues).toBe(issues);
    expect(now.linkedTickets["local-task"]?.status).toBe("Closed");
    expect(now.linkedTickets["local-task"]?.context_warning).toContain("agent context");
  } finally {
    older.resolve([]);
    await Promise.allSettled([poll, nudge]);
    load.mockRestore();
    useStore.setState({ tasks, selectedTask, linkedTickets });
  }
});

test("a late link event retains a newer matching status and adds its context warning", async () => {
  const { tasks, selectedTask, linkedTickets } = useStore.getState();
  const url = "https://acme.example/browse/ACME-42";
  const saved: TaskView = {
    id: "local-task",
    name: "Fix login",
    root: "/tmp/local-task",
    branch: "fix-login",
    issue_key: "ACME-42",
    issue_url: "https://ACME.example:443/browse/ACME-42",
    created_at: "2026-11-01T12:00:00Z",
    checkouts: [],
    pane_count: 0,
  };
  const link = {
    task_id: saved.id,
    key: "ACME-42",
    url,
    status: "Open",
    status_category: "new",
    context_warning: "Context write failed.",
  };
  const load = spyOn(api, "listTasks").mockResolvedValue([saved]);
  useStore.setState({
    tasks: [saved],
    linkedTickets: {
      [saved.id]: {
        ...link,
        url: "https://Acme.example/browse/ACME-42",
        status: "Done",
        status_category: "done",
        context_warning: null,
      },
    },
  });
  try {
    await useStore.getState().taskTicketLinked(link);
    expect(useStore.getState().linkedTickets[saved.id]).toMatchObject({
      status: "Done",
      context_warning: "Context write failed.",
    });
  } finally {
    load.mockRestore();
    useStore.setState({ tasks, selectedTask, linkedTickets });
  }
});

test("refreshing Mine finishes before a held Work lookup publishes Done", async () => {
  const initial = useStore.getState();
  const url = "https://acme.example/browse/ACME-42";
  const mine: Ticket = {
    key: "ACME-42",
    url,
    summary: "Fix login",
    description: "",
    status: "In Progress",
    status_id: "3",
    status_category: "indeterminate",
    issue_type: "Task",
    priority: null,
    assignee: "Me",
    labels: [],
    components: [],
    epic_key: null,
    epic_summary: null,
    created: null,
    updated: null,
  };
  const link = {
    task_id: "local-task",
    key: "ACME-42",
    url,
    status: "To Do",
    status_category: "new",
    context_warning: "Context write failed.",
  };
  const settings = {
    ...initial.settings,
    tracker: "jira",
    tracker_chosen: "jira",
    jira: {
      base_url: "https://acme.example",
      email: "me@example.test",
      project_key: null,
      jql: null,
      flow: {},
    },
  } as Settings;
  const byKey = Promise.withResolvers<Ticket[]>();
  const started = Promise.withResolvers<void>();
  const reads = [
    spyOn(api, "ticketsMine").mockResolvedValue({ issues: [], more: false }),
    spyOn(api, "ticketsByKey").mockImplementation(() => {
      started.resolve();
      return byKey.promise;
    }),
    spyOn(api, "ticketTypes").mockResolvedValue([]),
    spyOn(api, "ticketPriorities").mockResolvedValue([]),
  ];
  useStore.setState({
    settings,
    issues: [mine],
    linkedTickets: { "local-task": link },
    issuesLoaded: true,
  });
  const refresh = useStore.getState().refreshIssues({ quiet: true });
  try {
    await Promise.race([
      started.promise,
      refresh.then(() => {
        throw Error("Linked ticket was not read independently");
      }),
    ]);
    expect(await Promise.race([refresh.then(() => true), Bun.sleep(0).then(() => false)])).toBe(
      true,
    );
    expect(useStore.getState().issues).toEqual([]);
    expect(useStore.getState().linkedTickets["local-task"]?.status).toBe("To Do");
    byKey.resolve([
      {
        ...mine,
        url: "https://ACME.example:443/browse/ACME-42",
        status: "Done",
        status_id: "5",
        status_category: "done",
      },
    ]);
    // Let the detached request publish before asserting on its result.
    await Bun.sleep(0);
    expect(useStore.getState().issues).toEqual([]);
    expect(useStore.getState().linkedTickets["local-task"]).toMatchObject({
      status: "Done",
      status_category: "done",
      context_warning: "Context write failed.",
    });
  } finally {
    byKey.resolve([]);
    await refresh;
    await Bun.sleep(0);
    for (const read of reads) read.mockRestore();
    useStore.setState(initial);
  }
});

test("Work reads the same GitHub ticket across host case and a default port", () => {
  const settings = {
    tracker: "github",
    tracker_chosen: "github",
    github: {
      api_url: "https://api.github.com",
      web_url: "https://GITHUB.COM:443",
      review_team: null,
    },
  } as Settings;
  expect(readsTicket(settings, "acme/web#12", "https://github.com/acme/web/issues/12")).toBe(true);
});

test("Work keeps tracker base paths, protocols and non-default ports distinct", () => {
  const settings = {
    tracker: "jira",
    tracker_chosen: "jira",
    jira: {
      base_url: "https://acme.example:8443/team-a",
      email: "me@example.test",
      project_key: null,
      jql: null,
      flow: {},
    },
  } as Settings;
  expect(readsTicket(settings, "ACME-42", "https://acme.example:8443/team-a/browse/ACME-42")).toBe(
    true,
  );
  expect(readsTicket(settings, "ACME-42", "https://acme.example:8443/team-b/browse/ACME-42")).toBe(
    false,
  );
  expect(readsTicket(settings, "ACME-42", "https://acme.example/team-a/browse/ACME-42")).toBe(
    false,
  );
  expect(readsTicket(settings, "ACME-42", "http://acme.example:8443/team-a/browse/ACME-42")).toBe(
    false,
  );
});
