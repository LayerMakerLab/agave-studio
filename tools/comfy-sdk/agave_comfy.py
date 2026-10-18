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

    submit --workflow <api-format.json> [--image-node <id> --image <picture>]   -> {"job_id": "..."}
    status <job_id>     -> {"status": "...", "outputs": [{"name", "content_type", "node_id", "type"}], "error": ...}
    fetch <job_id> --name <output name> --out <path> --max-bytes <n>          -> {"bytes": n, "content_type": "..."}
"""

from __future__ import annotations

import argparse
import json
import os
import sys


def fail(message: str, code: int = 1) -> None:
    print(message, file=sys.stderr)
    sys.exit(code)


def client():
    base = os.environ.get("COMFY_BASE_URL", "").strip()
    # Never fall back to Comfy Cloud (the SDK's default when COMFY_BASE_URL is empty), and never send a key.
    if not base.startswith(("http://", "https://")):
        fail("COMFY_BASE_URL must be set to your ComfyUI address (http:// or https://)", 2)
    os.environ.pop("COMFY_API_KEY", None)
    from comfy_sdk import Comfy

    return Comfy(timeout=60, client_info="agave-studio")


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
    if args.image:
        if not args.image_node:
            fail("--image needs --image-node")
        wf.set_input(args.image_node, "image", c.assets.from_file(args.image))
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


def main() -> None:
    p = argparse.ArgumentParser(prog="agave_comfy")
    sub = p.add_subparsers(dest="command", required=True)
    s = sub.add_parser("submit")
    s.add_argument("--workflow", required=True)
    s.add_argument("--image-node")
    s.add_argument("--image")
    st = sub.add_parser("status")
    st.add_argument("job_id")
    f = sub.add_parser("fetch")
    f.add_argument("job_id")
    f.add_argument("--name", required=True)
    f.add_argument("--out", required=True)
    f.add_argument("--max-bytes", type=int, required=True)
    args = p.parse_args()
    run = {"submit": cmd_submit, "status": cmd_status, "fetch": cmd_fetch}[args.command]
    try:
        result = run(args)
    except SystemExit:
        raise
    except Exception as e:  # the gateway shows this text; keep it short and on stderr
        fail(f"{type(e).__name__}: {str(e)[:300]}")
    print(json.dumps(result))


if __name__ == "__main__":
    main()
