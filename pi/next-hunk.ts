/**
 * next-hunk extension for the pi coding agent.
 *
 * Wraps the `nh` CLI so the agent can drive a next-hunk review session the
 * way the bundled skill describes: inspect the diff, guide the human's
 * attention (navigate / push / comment / highlight), refresh after edits,
 * and read back per-hunk decisions.
 *
 * Install: copy to ~/.pi/agent/extensions/next-hunk.ts, drop it in
 * <project>/.pi/extensions/, or run `pi -e ./pi/next-hunk.ts`.
 * Requires the `nh` (or `next-hunk`) binary on PATH, or NEXT_HUNK_BIN.
 *
 * Targets pi >= 0.84 (@earendil-works/pi-coding-agent). No dependencies
 * beyond pi's own packages.
 */
import { StringEnum, Type } from "@earendil-works/pi-ai";
import { defineTool, type ExtensionAPI } from "@earendil-works/pi-coding-agent";

const MAX_OUTPUT_CHARS = 30_000;

interface ExecResult {
	stdout: string;
	stderr: string;
	code: number;
	killed: boolean;
}

function truncate(text: string, maxChars = MAX_OUTPUT_CHARS): string {
	if (text.length <= maxChars) {
		return text;
	}
	return `${text.slice(0, maxChars)}\n…[truncated, ${text.length - maxChars} more chars]`;
}

function textResult(text: string) {
	return {
		content: [{ type: "text" as const, text: truncate(text) }],
		details: {} as Record<string, unknown>,
	};
}

/** Friendly, actionable hint when a session-dependent command found nothing. */
function sessionHint(stderr: string): string | null {
	const s = stderr.toLowerCase();
	if (
		s.includes("no live next-hunk session") ||
		s.includes("session went away") ||
		s.includes("connect to server socket")
	) {
		return (
			"No live next-hunk review session in this repo. " +
			"Ask the user to open one in another terminal with `nh diff` (or `nh serve`), " +
			"then retry; `nh list` shows live sessions."
		);
	}
	return null;
}

function requireInt(value: number | undefined, name: string): number {
	if (value === undefined || !Number.isInteger(value)) {
		throw new Error(`Parameter \`${name}\` must be an integer`);
	}
	return value;
}

export default function nextHunkExtension(pi: ExtensionAPI): void {
	let resolvedBin: string | null = null;

	function binCandidates(): string[] {
		const override = process.env.NEXT_HUNK_BIN?.trim();
		if (override) {
			return [override];
		}
		const rest = ["nh", "next-hunk"].filter((b) => b !== resolvedBin);
		return resolvedBin ? [resolvedBin, ...rest] : rest;
	}

	/** Run the nh CLI, falling back across binary names; throw guidance when absent. */
	async function runNh(args: string[], signal?: AbortSignal): Promise<ExecResult> {
		for (const bin of binCandidates()) {
			let res: ExecResult;
			try {
				res = await pi.exec(bin, args, { timeout: 30_000, signal });
			} catch {
				continue; // spawn failure (binary missing) — try the next name
			}
			if (res.code === 0 || res.stdout.trim() !== "" || res.stderr.trim() !== "") {
				resolvedBin = bin;
				return res;
			}
			// Nonzero with no output reads like a spawn failure; try the next name.
		}
		throw new Error(
			"next-hunk binary not found. Install it (`cargo install next-hunk`, or a release " +
				"binary from https://github.com/wuxiaobai24/next-hunk) so `nh` is on PATH, " +
				"or set NEXT_HUNK_BIN to the binary path.",
		);
	}

	/** Run a session command; expected "no session" outcomes become hints, not errors. */
	async function runSession(args: string[], signal?: AbortSignal) {
		const res = await runNh(args, signal);
		if (res.code !== 0) {
			const hint = sessionHint(res.stderr);
			if (hint) {
				return { hint, output: "" };
			}
			throw new Error(res.stderr.trim() || `nh ${args[0]} failed with exit code ${res.code}`);
		}
		return { hint: null, output: res.stdout };
	}

	function hashArgs(session: string | undefined): string[] {
		return session ? ["--hash", session] : [];
	}

	function commandDetails(bin: string | null, args: string[]): Record<string, unknown> {
		return { command: [bin ?? "nh", ...args].join(" ") };
	}

	const nhInspect = defineTool({
		name: "nh_inspect",
		label: "next-hunk inspect",
		description:
			"Summarize the current diff of this repo (per-file row/byte counts) without opening " +
			"any UI. Use it after editing files to sanity-check what changed, or anytime you want " +
			"a quick overview of the working-tree (or staged) diff. Non-interactive; no review " +
			"session needed.",
		parameters: Type.Object({
			staged: Type.Optional(
				Type.Boolean({ description: "Diff the staged changes (git diff --cached) instead of the worktree" }),
			),
		}),
		async execute(_toolCallId, params, signal) {
			const args = ["inspect", ...(params.staged ? ["--staged"] : [])];
			const res = await runNh(args, signal);
			if (res.code !== 0) {
				throw new Error(res.stderr.trim() || "nh inspect failed");
			}
			return {
				content: [{ type: "text" as const, text: truncate(res.stdout) }],
				details: commandDetails(resolvedBin, args),
			};
		},
	});

	const nhSessions = defineTool({
		name: "nh_sessions",
		label: "next-hunk sessions",
		description:
			"List live next-hunk review TUI sessions (interactive `nh diff` / `nh serve` windows " +
			"the human has open). Start here whenever you want to work with a review: it shows " +
			"which sessions exist, which repo each belongs to, and where the human is looking. " +
			"The session id prefix can be passed as `session` to the other nh_* tools.",
		parameters: Type.Object({}),
		async execute(_toolCallId, _params, signal) {
			const list = await runNh(["list"], signal);
			if (list.code !== 0) {
				throw new Error(list.stderr.trim() || "nh list failed");
			}
			const raw = list.stdout.trim();
			if (raw === "" || raw.includes("no live sessions found")) {
				return textResult(
					"No live next-hunk sessions anywhere on this machine. " +
						"Ask the user to open one with `nh diff` in another terminal if they want an interactive review.",
				);
			}
			// Mark the current repo's session (if any) using `nh get`, which
			// resolves this repo's socket without needing our cwd.
			const get = await runNh(["get"], signal);
			let currentSocket: string | null = null;
			if (get.code === 0) {
				currentSocket = /^socket:\s+(.+)$/m.exec(get.stdout)?.[1]?.trim() ?? null;
			}
			const lines = raw.split("\n").map((line) => {
				const socket = line.trimEnd().split(/\s{2,}/).pop()?.trim();
				return currentSocket && socket && currentSocket === socket ? `${line}  <- this repo` : line;
			});
			return textResult(lines.join("\n"));
		},
	});

	const nhReview = defineTool({
		name: "nh_review",
		label: "next-hunk review",
		description:
			"Dump the structure of the diff shown in the running next-hunk session as JSON: files, " +
			"per-file insert/delete counts, and every hunk's header and line range. Use it to see " +
			"exactly what the human is reviewing before navigating, commenting, or reading decisions.",
		parameters: Type.Object({
			session: Type.Optional(
				Type.String({ description: "Session id prefix from nh_sessions; omit for this repo's session" }),
			),
		}),
		async execute(_toolCallId, params, signal) {
			const args = ["review", ...hashArgs(params.session)];
			const { hint, output } = await runSession(args, signal);
			if (hint) {
				return textResult(hint);
			}
			let text = output;
			try {
				text = JSON.stringify(JSON.parse(output));
			} catch {
				// keep raw output
			}
			return {
				content: [{ type: "text" as const, text: truncate(text) }],
				details: commandDetails(resolvedBin, args),
			};
		},
	});

	const nhContext = defineTool({
		name: "nh_context",
		label: "next-hunk context",
		description:
			"Report where the human is currently looking in the running next-hunk session: focused " +
			"file, 1-based hunk ordinal, and source line, as JSON. Use it to follow the human's " +
			"attention, e.g. before commenting on what they are staring at.",
		parameters: Type.Object({
			session: Type.Optional(
				Type.String({ description: "Session id prefix from nh_sessions; omit for this repo's session" }),
			),
		}),
		async execute(_toolCallId, params, signal) {
			const args = ["context", "--json", ...hashArgs(params.session)];
			const { hint, output } = await runSession(args, signal);
			return textResult(hint ?? output.trim());
		},
	});

	const nhNavigate = defineTool({
		name: "nh_navigate",
		label: "next-hunk navigate",
		description:
			"Scroll the human's next-hunk review session to a specific place: a file, a file:line, " +
			"or a file's n-th hunk. Use it to walk the human through your changes step by step, or " +
			"to jump between annotated rows.",
		parameters: Type.Object({
			target: Type.Optional(
				Type.String({
					description:
						"Where to scroll: `<path>`, `<path>:<line>`, or `<path>:h<n>` (1-based hunk ordinal), e.g. `src/main.rs:h2`",
				}),
			),
			direction: Type.Optional(
				StringEnum(["next_note", "prev_note"], {
					description: "Instead of an explicit target, jump to the next/previous annotated (comment) row",
				}),
			),
			session: Type.Optional(
				Type.String({ description: "Session id prefix from nh_sessions; omit for this repo's session" }),
			),
		}),
		async execute(_toolCallId, params, signal) {
			if (!!params.target === !!params.direction) {
				throw new Error("Give exactly one of `target` or `direction`");
			}
			const args = ["navigate"];
			if (params.target) {
				args.push(params.target);
			} else {
				args.push(params.direction === "prev_note" ? "--prev-note" : "--next-note");
			}
			args.push(...hashArgs(params.session));
			const { hint, output } = await runSession(args, signal);
			return textResult(hint ?? output.trim());
		},
	});

	const nhComment = defineTool({
		name: "nh_comment",
		label: "next-hunk comment",
		description:
			"Manage review comments on the diff in the running next-hunk session; they render live " +
			"in the human's TUI as notes. Use `add` to explain a change, ask a question, or record " +
			"a review finding; `list` to see what has been said (yours and the human's); `rm` to " +
			"delete one of yours; `clear` to wipe them.",
		parameters: Type.Object({
			action: StringEnum(["add", "list", "rm", "clear"], { description: "What to do" }),
			text: Type.Optional(Type.String({ description: "Comment text (action=add)" })),
			file: Type.Optional(Type.String({ description: "Target file path (add/rm/clear)" })),
			line: Type.Optional(Type.Integer({ description: "New-side source line number (add; alternative to hunk)" })),
			hunk: Type.Optional(Type.Integer({ description: "1-based hunk ordinal (add; alternative to line)" })),
			focus: Type.Optional(
				Type.Boolean({ description: "Also scroll the human's view to the new comment (add, default false)" }),
			),
			id: Type.Optional(Type.String({ description: "Comment id (`cN`) to remove (action=rm)" })),
			all: Type.Optional(
				Type.Boolean({ description: "Clear everything including human-authored notes (action=clear; needs this or file)" }),
			),
			session: Type.Optional(
				Type.String({ description: "Session id prefix from nh_sessions; omit for this repo's session" }),
			),
		}),
		async execute(_toolCallId, params, signal) {
			const args = ["comment"];
			switch (params.action) {
				case "add": {
					if (!params.text || !params.file) {
						throw new Error("action=add needs `file` and `text`");
					}
					if (params.line !== undefined && params.hunk !== undefined) {
						throw new Error("Give either `line` or `hunk`, not both");
					}
					args.push("add", "--file", params.file);
					if (params.line !== undefined) args.push("--line", String(requireInt(params.line, "line")));
					if (params.hunk !== undefined) args.push("--hunk", String(requireInt(params.hunk, "hunk")));
					if (params.focus) args.push("--focus");
					args.push(params.text);
					break;
				}
				case "list":
					args.push("list");
					break;
				case "rm": {
					if (!params.id) {
						throw new Error("action=rm needs `id`");
					}
					args.push("rm", params.id);
					break;
				}
				case "clear": {
					if (!params.file && !params.all) {
						throw new Error("action=clear needs `file` or all=true");
					}
					args.push("clear", "--yes");
					if (params.file) args.push("--file", params.file);
					if (params.all) args.push("--all");
					break;
				}
			}
			args.push(...hashArgs(params.session));
			const { hint, output } = await runSession(args, signal);
			return textResult(hint ?? output.trim());
		},
	});

	const nhHighlight = defineTool({
		name: "nh_highlight",
		label: "next-hunk highlight",
		description:
			"Paint an attention mark over a character range of one diff line in the running " +
			"next-hunk session — \"look at exactly these columns\". Use it to point out a specific " +
			"token or expression the human should not miss.",
		parameters: Type.Object({
			action: StringEnum(["add", "list", "clear"], { description: "What to do" }),
			file: Type.Optional(Type.String({ description: "Target file path (add/clear)" })),
			line: Type.Optional(Type.Integer({ description: "New-side source line number (add)" })),
			start: Type.Optional(Type.Integer({ description: "1-based start char index, inclusive (add)" })),
			end: Type.Optional(Type.Integer({ description: "1-based end char index, exclusive (add)" })),
			tone: Type.Optional(
				StringEnum(["warning", "danger", "info", "accent"], { description: "Mark color (add, default warning)" }),
			),
			focus: Type.Optional(
				Type.Boolean({ description: "Also scroll the human's view to the marked line (add, default false)" }),
			),
			session: Type.Optional(
				Type.String({ description: "Session id prefix from nh_sessions; omit for this repo's session" }),
			),
		}),
		async execute(_toolCallId, params, signal) {
			const args = ["highlight"];
			switch (params.action) {
				case "add": {
					if (
						params.file === undefined ||
						params.line === undefined ||
						params.start === undefined ||
						params.end === undefined
					) {
						throw new Error("action=add needs `file`, `line`, `start`, and `end`");
					}
					if (params.end <= params.start) {
						throw new Error("`end` must be greater than `start`");
					}
					args.push(
						"add",
						"--file", params.file,
						"--line", String(requireInt(params.line, "line")),
						"--start", String(requireInt(params.start, "start")),
						"--end", String(requireInt(params.end, "end")),
					);
					if (params.tone) args.push("--tone", params.tone);
					if (params.focus) args.push("--focus");
					break;
				}
				case "list":
					args.push("list");
					break;
				case "clear":
					args.push("clear");
					if (params.file) args.push("--file", params.file);
					break;
			}
			args.push(...hashArgs(params.session));
			const { hint, output } = await runSession(args, signal);
			return textResult(hint ?? output.trim());
		},
	});

	const nhPush = defineTool({
		name: "nh_push",
		label: "next-hunk push",
		description:
			"Push a focus hint and/or annotation notes into the human's running next-hunk session. " +
			"Use it to point the human at what matters right now — a file, hunk, or line plus a " +
			"one-line banner explaining why.",
		parameters: Type.Object({
			focus: Type.Optional(
				Type.String({
					description: "Where to scroll: `<path>`, `<path>:<line>`, or `<path>:h<n>`",
				}),
			),
			notes: Type.Optional(
				Type.Array(Type.String(), {
					description:
						"Annotations in nh note syntax: `<path>:<line>=<text>`, `<path>:h<n>=<text>`, or `banner=<text>`",
				}),
			),
			session: Type.Optional(
				Type.String({ description: "Session id prefix from nh_sessions; omit for this repo's session" }),
			),
		}),
		async execute(_toolCallId, params, signal) {
			if (!params.focus && (!params.notes || params.notes.length === 0)) {
				throw new Error("Give `focus`, `notes`, or both");
			}
			const args = ["push"];
			if (params.focus) args.push("--focus", params.focus);
			for (const note of params.notes ?? []) {
				args.push("--note", note);
			}
			args.push(...hashArgs(params.session));
			const { hint, output } = await runSession(args, signal);
			return textResult(hint ?? output.trim());
		},
	});

	const nhReload = defineTool({
		name: "nh_reload",
		label: "next-hunk reload",
		description:
			"Tell the running next-hunk session to re-read its diff so the human's TUI reflects " +
			"your latest edits. Call this after changing files while a review session is open; " +
			"focus, notes, and decisions are preserved best-effort.",
		parameters: Type.Object({
			session: Type.Optional(
				Type.String({ description: "Session id prefix from nh_sessions; omit for this repo's session" }),
			),
		}),
		async execute(_toolCallId, params, signal) {
			const args = ["reload", ...hashArgs(params.session)];
			const { hint, output } = await runSession(args, signal);
			return textResult(hint ?? output.trim());
		},
	});

	const nhDecision = defineTool({
		name: "nh_decision",
		label: "next-hunk decision",
		description:
			"Read the human's per-hunk verdicts from the running next-hunk session as one JSON " +
			'line: {"accepted":[…],"rejected":[…],"undecided":[…]}. Decisions exist only in ' +
			"sessions where the human marks hunks (a/r/u — serve and --select do). Check this " +
			"after giving the human time to review, or whenever you need their accept/reject calls.",
		parameters: Type.Object({
			session: Type.Optional(
				Type.String({ description: "Session id prefix from nh_sessions; omit for this repo's session" }),
			),
		}),
		async execute(_toolCallId, params, signal) {
			const args = ["decision", ...hashArgs(params.session)];
			const { hint, output } = await runSession(args, signal);
			return textResult(hint ?? output.trim());
		},
	});

	pi.registerTool(nhInspect);
	pi.registerTool(nhSessions);
	pi.registerTool(nhReview);
	pi.registerTool(nhContext);
	pi.registerTool(nhNavigate);
	pi.registerTool(nhComment);
	pi.registerTool(nhHighlight);
	pi.registerTool(nhPush);
	pi.registerTool(nhReload);
	pi.registerTool(nhDecision);

	pi.registerCommand("nh", {
		description: "next-hunk review status (usage: /nh [decision|comments])",
		getArgumentCompletions: (prefix) => {
			const options = ["decision", "comments"];
			return options
				.filter((o) => o.startsWith(prefix))
				.map((o) => ({ value: o, label: o }));
		},
		handler: async (args, ctx) => {
			const arg = args.trim();
			try {
				if (arg === "decision") {
					const res = await runNh(["decision"]);
					await ctx.ui.notify(res.code === 0 ? res.stdout.trim() : res.stderr.trim(), "info");
					return;
				}
				if (arg === "comments") {
					const res = await runNh(["comment", "list"]);
					await ctx.ui.notify(res.code === 0 ? res.stdout.trim() : res.stderr.trim(), "info");
					return;
				}
				const list = await runNh(["list"]);
				const msg = list.code === 0 && list.stdout.trim() !== "" && !list.stdout.includes("no live sessions found")
					? `next-hunk sessions:\n${list.stdout.trim()}`
					: "No live next-hunk sessions. Open one with `nh diff` in another terminal.";
				await ctx.ui.notify(msg, "info");
			} catch (err) {
				await ctx.ui.notify(err instanceof Error ? err.message : String(err), "warning");
			}
		},
	});

	// Surface an already-running review session when pi starts, so the model
	// (and human) immediately see that nh_* tools have something to talk to.
	pi.on("session_start", async (_event, ctx) => {
		try {
			const list = await runNh(["list"]);
			const raw = list.stdout.trim();
			if (list.code !== 0 || raw === "" || raw.includes("no live sessions found")) {
				return;
			}
			const get = await runNh(["get"]);
			const socket = get.code === 0 ? /^socket:\s+(.+)$/m.exec(get.stdout)?.[1]?.trim() : null;
			const mine = raw
				.split("\n")
				.find((line) => socket && line.trimEnd().split(/\s{2,}/).pop()?.trim() === socket);
			if (mine) {
				ctx.ui.setWidget("next-hunk", [`next-hunk review live: ${mine.split(/\s{2,}/)[0]}`]);
				ctx.ui.notify("A next-hunk review session is open in this repo — the nh_* tools can inspect and drive it.", "info");
			}
		} catch {
			// binary missing or transient failure — stay quiet
		}
	});
}
