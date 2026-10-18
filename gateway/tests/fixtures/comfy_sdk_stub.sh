#!/bin/sh
# Test stand-in for tools/comfy-sdk/agave_comfy.py (unit tests in src/providers.rs). Prints canned JSON and
# never opens a network connection. $1 picks the behaviour, $2 is the command, the rest are its arguments.
mode="$1"; cmd="$2"; shift 2
[ -n "$COMFY_BASE_URL" ] || { echo "no COMFY_BASE_URL" >&2; exit 2; }
[ -z "$COMFY_API_KEY" ] || { echo "a key reached the client" >&2; exit 2; }
case "$mode" in
  fail) echo "boom: the proxy said no" >&2; exit 3 ;;
  garbage) echo "not json"; exit 0 ;;
  slow) sleep 5; echo '{}'; exit 0 ;;
  forkslow) sleep 30 & echo $! > "$PWD/grandchild.pid"; sleep 30; exit 0 ;;
  flood) i=0; while [ $i -lt 5000 ]; do echo "xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx"; i=$((i+1)); done; exit 0 ;;
esac
inside() { case "$1" in "$PWD"/*) return 0 ;; *) echo "path outside the work folder: $1" >&2; exit 5 ;; esac; }
case "$cmd" in
  submit)
    wf=""; node=""; img=""
    while [ $# -gt 0 ]; do
      case "$1" in --workflow=*) wf="${1#*=}" ;; --image-node=*) node="${1#*=}" ;; --image=*) img="${1#*=}" ;; esac; shift
    done
    inside "$wf"; [ -f "$wf" ] || { echo "no workflow file" >&2; exit 4; }
    if [ -n "$img" ]; then inside "$img"; [ -f "$img" ] || { echo "no picture" >&2; exit 4; }; fi
    [ "$node" = "78" ] || { echo "picture for node '$node'" >&2; exit 4; }
    if [ "$mode" = oddid ]; then echo '{"job_id": "../../etc"}'; else echo '{"job_id": "11111111-2222-4333-a444-555555555555"}'; fi ;;
  status)
    [ "$1" = "--" ] || { echo "no -- before the job id" >&2; exit 4; }; shift
    [ "$1" = "11111111-2222-4333-a444-555555555555" ] || { echo "status for $1" >&2; exit 4; }
    case "$mode" in
      running) echo '{"status": "running", "outputs": [], "error": null}' ;;
      badname) echo '{"status": "succeeded", "outputs": [{"name": "../../secret.mp4", "content_type": "video/mp4", "node_id": "81", "type": "image"}], "error": null}' ;;
      failedjob) echo '{"status": "failed", "outputs": [], "error": "out of memory"}' ;;
      *) echo '{"status": "succeeded", "outputs": [{"name": "video/agave_00001_.mp4", "content_type": "video/mp4", "node_id": "81", "type": "image"}], "error": null}' ;;
    esac ;;
  fetch)
    out=""; name=""
    while [ $# -gt 0 ]; do
      case "$1" in --out=*) out="${1#*=}"; shift ;; --name=*) name="${1#*=}"; shift ;; --) shift; job="$1"; shift ;; *) shift ;; esac
    done
    inside "$out"
    [ "$job" = "11111111-2222-4333-a444-555555555555" ] || { echo "fetch for '$job'" >&2; exit 4; }
    [ "$name" = "video/agave_00001_.mp4" ] || { echo "wrong output name: $name" >&2; exit 4; }
    if [ "$mode" = big ]; then printf 'this file is far too large' > "$out"; else printf 'fakemp4' > "$out"; fi
    echo '{"bytes": 7, "content_type": "video/mp4"}' ;;
  *) echo "unknown command $cmd" >&2; exit 4 ;;
esac
