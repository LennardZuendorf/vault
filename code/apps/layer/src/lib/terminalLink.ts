import { safeHref } from "./markdown";

/**
 * A click on a link in a terminal (PANE-23): a web or mail link opens in
 * the browser, at once. xterm's own handler asked "Do you want to navigate
 * to…?" and then called `window.open`, which does nothing in the app's
 * window: the user said yes, and nothing happened. Anything else (a
 * `file://` link, a custom scheme) is not opened, and says so, rather than
 * running whatever the Mac would hand it to.
 */
export function openTerminalLink(
  uri: string,
  open: (url: string) => Promise<void>,
  fail: (text: string) => void,
): void {
  if (!safeHref(uri)) {
    fail(`Only web and mail links open from a terminal. Copy this one instead: ${uri}`);
    return;
  }
  void open(uri).catch(() => fail("Couldn't open the browser. Try again."));
}
