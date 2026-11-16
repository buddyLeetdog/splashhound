"""Drive the running InflaTool WebView2 through the Chrome DevTools Protocol.

Usage: python tools/ui_probe.py <step> ...
Steps are executed in order, e.g.:
  python tools/ui_probe.py eval "document.title" shot library.png click "#f-tags-btn"
"""

import base64
import json
import sys
import time
import urllib.request

import websocket

PORT = 9222


def target_url():
    with urllib.request.urlopen(f"http://127.0.0.1:{PORT}/json/list", timeout=10) as resp:
        targets = json.load(resp)
    pages = [t for t in targets if t.get("type") == "page"]
    if not pages:
        raise SystemExit("no page target")
    return pages[0]["webSocketDebuggerUrl"]


class Cdp:
    def __init__(self, url):
        self.ws = websocket.create_connection(url, timeout=30)
        self.next_id = 1
        self.events = []

    def send(self, method, params=None, timeout=60):
        msg_id = self.next_id
        self.next_id += 1
        self.ws.send(json.dumps({"id": msg_id, "method": method, "params": params or {}}))
        deadline = time.time() + timeout
        while time.time() < deadline:
            raw = self.ws.recv()
            msg = json.loads(raw)
            if msg.get("id") == msg_id:
                return msg
            self.events.append(msg)
        raise TimeoutError(method)

    def evaluate(self, expression, await_promise=True):
        res = self.send(
            "Runtime.evaluate",
            {
                "expression": expression,
                "awaitPromise": await_promise,
                "returnByValue": True,
                "userGesture": True,
            },
        )
        result = res.get("result", {})
        if "exceptionDetails" in result:
            return {"__exception": result["exceptionDetails"]}
        return result.get("result", {}).get("value")

    def screenshot(self, path):
        res = self.send("Page.captureScreenshot", {"format": "png", "captureBeyondViewport": False})
        data = res.get("result", {}).get("data")
        if not data:
            print("screenshot failed", res)
            return
        with open(path, "wb") as fh:
            fh.write(base64.b64decode(data))
        print("screenshot ->", path)


def main():
    sys.stdout.reconfigure(encoding="utf-8", errors="replace")
    cdp = Cdp(target_url())
    cdp.send("Runtime.enable")
    cdp.send("Page.enable")
    cdp.send("Log.enable")

    args = sys.argv[1:]
    i = 0
    while i < len(args):
        step = args[i]
        if step == "eval":
            expr = args[i + 1]
            i += 2
            value = cdp.evaluate(expr)
            print(json.dumps(value, ensure_ascii=False, indent=2))
        elif step == "click":
            sel = args[i + 1]
            i += 2
            value = cdp.evaluate(
                "(() => { const el = document.querySelector(%s); if (!el) return 'missing';"
                " el.scrollIntoView({block:'center'}); el.click(); return 'clicked'; })()" % json.dumps(sel)
            )
            print("click", sel, "->", value)
        elif step == "shot":
            path = args[i + 1]
            i += 2
            cdp.screenshot(path)
        elif step == "wait":
            secs = float(args[i + 1])
            i += 2
            time.sleep(secs)
        else:
            raise SystemExit(f"unknown step {step}")

    errors = [
        e for e in cdp.events
        if e.get("method") in ("Runtime.exceptionThrown", "Log.entryAdded")
    ]
    if errors:
        print("--- console/exceptions ---")
        for e in errors[-20:]:
            print(json.dumps(e.get("params", {}), ensure_ascii=False)[:1200])


if __name__ == "__main__":
    main()
