#!/usr/bin/env python3
"""End-to-end smoke test for `next-hunk serve` + `push` + `decision`.

Spawns `serve` in a pty (so its tty check passes), waits for the socket to
appear, then exercises the push/decision client commands from a second
process and asserts their output. Sends `q` to quit the serve cleanly.

Run from a git repo with an uncommitted diff:
    python3 scripts/smoke_serve.py [path/to/next-hunk]
"""
import os, pty, select, subprocess, sys, time, errno

BIN = sys.argv[1] if len(sys.argv) > 1 else "./target/release/next-hunk"
REPO = os.getcwd()


def read_avail(fd, timeout=0.5):
    chunks = []
    end = time.time() + timeout
    while time.time() < end:
        r, _, _ = select.select([fd], [], [], max(0.0, end - time.time()))
        if not r:
            break
        try:
            data = os.read(fd, 4096)
        except OSError as e:
            if e.errno == errno.EIO:
                break
            raise
        if not data:
            break
        chunks.append(data)
    return b"".join(chunks)


def main():
    pid, fd = pty.fork()
    if pid == 0:
        # child
        os.execv(BIN, [BIN, "serve"])
        os._exit(127)

    # parent: wait for the TUI to come up (look for any output)
    boot = read_avail(fd, timeout=5)
    if not boot:
        print("FAIL: serve produced no output within 5s", file=sys.stderr)
        os.kill(pid, 9)
        sys.exit(1)

    # --- push ---
    push = subprocess.run(
        [BIN, "push", "--note", "banner=smoke-test"],
        capture_output=True, text=True,
    )
    print(f"push stdout: {push.stdout.strip()!r}  exit={push.returncode}")
    assert push.returncode == 0, f"push failed: {push.stderr}"
    assert "ok" in push.stdout, f"unexpected push output: {push.stdout!r}"

    # --- decision (should be one JSON line; all-undecided since human did nothing) ---
    dec = subprocess.run(
        [BIN, "decision"], capture_output=True, text=True,
    )
    print(f"decision stdout: {dec.stdout.strip()!r}  exit={dec.returncode}")
    assert dec.returncode == 0, f"decision failed: {dec.stderr}"
    import json
    parsed = json.loads(dec.stdout.strip())
    assert set(parsed.keys()) == {"accepted", "rejected", "undecided"}, parsed
    # human hasn't pressed anything yet → everything undecided, nothing accepted/rejected
    assert parsed["accepted"] == [] and parsed["rejected"] == [], parsed

    # --- quit the serve cleanly ---
    os.write(fd, b"q")
    time.sleep(0.5)
    read_avail(fd, timeout=1)  # drain
    _, status = os.waitpid(pid, 0)
    code = os.waitstatus_to_exitcode(status)
    print(f"serve exited with code {code}")

    print("PASS: serve + push + decision round-trip OK")
    sys.exit(0 if code == 0 else 1)


if __name__ == "__main__":
    main()
