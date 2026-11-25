import { describe, expect, test } from "bun:test";
import { downloadText } from "../src/components/BrowserDownload";

describe("the browser's download (BRW-24)", () => {
  test("its size is counted in megabytes as Finder counts them", () => {
    // Chrome for Testing 158's zip: 200 MB in Finder, 191 in mebibytes.
    expect(downloadText(84_000_000, 200_337_306)).toBe("Downloading the browser: 84 of 200 MB");
  });

  test("nothing come yet says no numbers", () => {
    expect(downloadText(0, 200_337_306)).toBe("Downloading the browser");
  });

  test("a download of unknown size says how much has come", () => {
    expect(downloadText(5_000_000, 0)).toBe("Downloading the browser: 5 MB");
  });
});
