import { describe, expect, it } from "vitest";

import { thumbBase } from "./lib/api/backend";

describe("thumbBase", () => {
  it("uses http://thumb.localhost on WebView2 and Android", () => {
    const edge = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 Chrome/130.0.0.0 Edg/130.0.0.0";
    expect(thumbBase(edge)).toBe("http://thumb.localhost/");
    expect(thumbBase("Mozilla/5.0 (Linux; Android 14) AppleWebKit/537.36")).toBe("http://thumb.localhost/");
  });

  it("keeps the custom scheme on WebKitGTK and WKWebView", () => {
    expect(thumbBase("Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/605.1.15")).toBe("thumb://");
    expect(thumbBase("Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/605.1.15")).toBe("thumb://");
  });
});
