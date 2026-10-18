# /// script
# requires-python = ">=3.10"
# dependencies = ["comfy-sdk==0.5.0"]
# ///
"""Agave Studio's Comfy SDK client: runs ComfyUI jobs through the official Comfy SDK (comfy-sdk 0.5.0).

The gateway starts this with uv when the COMFY_CLIENT setting is "sdk":

    uv run --quiet --script tools/comfy-sdk/agave_comfy.py <command> ...

It talks only to the ComfyUI address in the COMFY_BASE_URL environment variable, which the gateway sets
(your own ComfyUI behind comfy-api-proxy; no key is sent). The gateway picks every file path. Each command prints
one JSON object on stdout; errors go to stderr with a non-zero exit.

    submit --workflow=<api-format.json> [--image-node=<id> --image=<picture>]...   -> {"job_id": "..."}
                        (one --image-node/--image pair per LoadImage picture, in order)
    status -- <job_id>  -> {"status": "...", "outputs": [{"name", "content_type", "node_id", "type"}], "error": ...}
    fetch --name=<output name> --out=<path> --max-bytes=<n> -- <job_id>       -> {"bytes": n, "content_type": "..."}
    selftest            -> {"ok": true}: the origin and redirect checks against a mock transport (no network)

Every request stays on COMFY_BASE_URL's origin (scheme, host and port): redirects are not followed, a 3xx answer is
an error, and a request to any other address is refused before it is sent.
"""

from __future__ import annotations

import argparse
import json
import os
import sys


def fail(message: str, code: int = 1) -> None:
    print(message, file=sys.stderr)
    sys.exit(code)


class OffOrigin(Exception):
    """A request or redirect that would leave the ComfyUI address."""


def origin(url) -> tuple[str, str, int]:
    import httpx

    u = httpx.URL(str(url))
    port = u.port or {"http": 80, "https": 443}.get(u.scheme, 0)
    return (u.scheme, u.host, port)


def http_client(base: str, transport=None):
    """The httpx client every SDK call goes through: no redirects, and nothing off the base origin."""
    import httpx

    want = origin(base)

    def on_request(request):
        if origin(request.url) != want:
            raise OffOrigin(f"refusing a request to another address ({request.url.scheme}://{request.url.host})")

    def on_response(response):
        if 300 <= response.status_code < 400:
            raise OffOrigin(f"refusing a redirect ({response.status_code}) from the ComfyUI address")

    kw = {"timeout": 60, "follow_redirects": False, "event_hooks": {"request": [on_request], "response": [on_response]}}
    if transport is not None:
        kw["transport"] = transport
    return httpx.Client(**kw)


def client(transport=None):
    base = os.environ.get("COMFY_BASE_URL", "").strip()
    # Never fall back to Comfy Cloud (the SDK's default when COMFY_BASE_URL is empty), and never send a key.
    if not base.startswith(("http://", "https://")):
        fail("COMFY_BASE_URL must be set to your ComfyUI address (http:// or https://)", 2)
    os.environ.pop("COMFY_API_KEY", None)
    from comfy_low import ComfyLow
    from comfy_sdk import Comfy
    from comfy_sdk.assets import AssetFactory
    from comfy_sdk.jobs import JobFactory

    c = Comfy(timeout=60, client_info="agave-studio")
    # comfy-sdk 0.5.0's Comfy takes no httpx client, and its transport builds one with follow_redirects=True
    # (comfy_low/transport.py:1025). ComfyLow's own `client=` parameter (transport.py:1018) is the supported way to
    # pass one in, so the client's namespaces are rebuilt on a ComfyLow that uses ours (pinned version, so the
    # layout below is fixed).
    low = ComfyLow(c._low.base_url, None, client=http_client(base, transport), client_info="agave-studio",
                   router_base_url=c._low.router_base_url)
    c._low.close()
    c._low = low
    c.assets = AssetFactory(low)
    c.jobs = JobFactory(low)
    return c


class CappedFile:
    """A binary file that refuses to grow past a byte limit (Output.to_stream writes into it)."""

    def __init__(self, path: str, limit: int) -> None:
        self.fh = open(path, "wb")
        self.left = limit

    def write(self, chunk: bytes) -> int:
        if len(chunk) > self.left:
            raise OSError("the output is larger than the studio accepts")
        self.left -= len(chunk)
        return self.fh.write(chunk)

    def close(self) -> None:
        self.fh.close()


def error_text(err) -> str | None:
    if err is None:
        return None
    if isinstance(err, dict):
        return str(err.get("message") or err.get("code") or err)[:300]
    return str(getattr(err, "message", None) or err)[:300]


def cmd_submit(args) -> dict:
    c = client()
    wf = c.workflows.from_file(args.workflow)
    nodes, images = args.image_node or [], args.image or []
    if len(nodes) != len(images):
        fail("each --image needs its --image-node")
    for node, path in zip(nodes, images):
        wf.set_input(node, "image", c.assets.from_file(path))
    job = c.submit(wf)
    return {"job_id": job.id}


def cmd_status(args) -> dict:
    job = client().jobs.get(args.job_id)
    outputs = [
        {"name": o.name, "content_type": o.content_type, "node_id": o.node_id, "type": o.type}
        for o in job.outputs
    ]
    return {"status": job.status, "outputs": outputs, "error": error_text(job.error)}


def cmd_fetch(args) -> dict:
    job = client().jobs.get(args.job_id)
    match = [o for o in job.outputs if o.name == args.name]
    if not match:
        fail("no output with that name on this job")
    out = CappedFile(args.out, args.max_bytes)
    try:
        written = match[0].to_stream(out)
    finally:
        out.close()
    return {"bytes": written, "content_type": match[0].content_type}


def cmd_selftest(_args) -> dict:
    """Checks the origin and redirect rules with httpx.MockTransport: nothing leaves this process."""
    import httpx

    base = "http://127.0.0.1:8189"
    seen = []
    job = "7f3d2c1b-9a8e-4d6f-b012-3c4d5e6f7a8b"
    asset = "9f8a1c0d-2b3e-4f56-8a7b-1c2d3e4f5a6b"
    now = "2026-10-18T00:00:00+00:00"

    def handler(request):
        seen.append(str(request.url))
        path = request.url.path
        if request.method == "POST" and path == "/api/v2/jobs":
            return httpx.Response(307, headers={"Location": "http://evil.test/api/v2/jobs"})
        if path == f"/api/v2/jobs/{job}":
            out = {"node_id": "81", "name": "video/agave_00001_.mp4", "type": "image", "content_type": "video/mp4",
                   "size_bytes": 7, "id": asset, "hash": None, "url": f"{base}/api/v2/assets/{asset}/content",
                   "url_expires_at": now, "job_id": job}
            return httpx.Response(200, json={
                "id": job, "status": "succeeded", "created_at": now, "started_at": now, "completed_at": now,
                "expires_at": now, "queue_position": None, "progress": None, "outputs": [out], "error": None,
                "urls": {"self": f"/api/v2/jobs/{job}", "events": "/e", "cancel": "/c"}})
        if path == f"/api/v2/assets/{asset}/content":
            return httpx.Response(302, headers={"Location": "http://127.0.0.1:11434/api/tags"})
        if path == "/api/v2/jobs/0b0b0b0b-1111-4222-a333-444444444444":
            # A same-origin redirect (to a path the proxy would serve) must be refused too.
            return httpx.Response(302, headers={"Location": f"{base}/api/v2/jobs/{job}"})
        return httpx.Response(404, json={"error": {"code": "not_found", "message": path}})

    os.environ["COMFY_BASE_URL"] = base
    c = client(transport=httpx.MockTransport(handler))
    checks = {}

    # 1. An asset download that redirects to another local service is refused, and nothing reaches it.
    j = c.jobs.get(job)
    import io

    try:
        j.outputs[0].to_stream(io.BytesIO())
        checks["download_redirect_refused"] = False
    except Exception as e:
        checks["download_redirect_refused"] = "redirect" in str(e) or isinstance(e, OffOrigin)
    # 2. A job POST answered with a 307 to another host is refused.
    wf = c.workflows.from_json({"1": {"class_type": "SaveImage", "inputs": {}}})
    try:
        c.submit(wf)
        checks["submit_redirect_refused"] = False
    except Exception as e:
        checks["submit_redirect_refused"] = "redirect" in str(e) or isinstance(e, OffOrigin)
    # 3. A redirect on the same address is refused as well: no 3xx is followed, wherever it points.
    # Both guards count: the client follows no redirects, and the 3xx check is what refuses the answer.
    checks["client_follows_no_redirects"] = c._low._client.follow_redirects is False
    before = len(seen)
    try:
        c.jobs.get("0b0b0b0b-1111-4222-a333-444444444444")
        checks["same_origin_redirect_refused"] = False
    except Exception as e:
        chain, err = [], e
        while err is not None:
            chain.append(err)
            err = err.__cause__ or err.__context__
        ours = any(isinstance(x, OffOrigin) or "refusing a redirect" in str(x) for x in chain)
        checks["same_origin_redirect_refused"] = ours and len(seen) == before + 1
    # 4. A request to any other address is refused before it is sent.
    try:
        c._low._client.get("http://192.168.1.5:8188/api/v2/jobs")
        checks["other_origin_refused"] = False
    except OffOrigin:
        checks["other_origin_refused"] = True
    checks["nothing_left_the_base"] = all(u.startswith(base + "/") for u in seen)
    checks["no_redirect_followed"] = not any("11434" in u or "evil.test" in u for u in seen)
    if not all(checks.values()):
        fail("selftest failed: " + json.dumps(checks))
    return {"ok": True, "checks": checks}


def main() -> None:
    p = argparse.ArgumentParser(prog="agave_comfy")
    sub = p.add_subparsers(dest="command", required=True)
    s = sub.add_parser("submit")
    s.add_argument("--workflow", required=True)
    s.add_argument("--image-node", action="append")
    s.add_argument("--image", action="append")
    st = sub.add_parser("status")
    st.add_argument("job_id")
    f = sub.add_parser("fetch")
    f.add_argument("job_id")
    sub.add_parser("selftest")
    f.add_argument("--name", required=True)
    f.add_argument("--out", required=True)
    f.add_argument("--max-bytes", type=int, required=True)
    args = p.parse_args()
    run = {"submit": cmd_submit, "status": cmd_status, "fetch": cmd_fetch, "selftest": cmd_selftest}[args.command]
    try:
        result = run(args)
    except SystemExit:
        raise
    except Exception as e:  # the gateway shows this text; keep it short and on stderr
        fail(f"{type(e).__name__}: {str(e)[:300]}")
    print(json.dumps(result))


if __name__ == "__main__":
    main()
