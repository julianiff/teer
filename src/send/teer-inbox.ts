/**
 * teer-inbox: lets teer, the terminal code-review tool, send this pi
 * session a message (a review tile's result, for the agent to apply).
 * teer installed this file (`s` in teer's unit view); delete it to stop.
 *
 * On session start it listens on a Unix socket only you can reach
 * (<tmpdir>/teer-pi-<uid>/<pid>.sock, in a 0700 folder) and registers it,
 * with a random key, in <pi dir>/teer-inbox/<pid>.json (0600). teer
 * connects, sends the key, then the message, which goes in as if you had
 * typed it, once the agent is done ("followUp"). On shutdown both go.
 * Node built-ins only.
 */
// teer-inbox v1

import * as crypto from "node:crypto";
import * as fs from "node:fs";
import * as net from "node:net";
import * as os from "node:os";
import * as path from "node:path";

type Ctx = {
	cwd: string;
	hasUI?: boolean;
	ui?: { notify(message: string, type?: "info" | "warning" | "error"): void };
	sessionManager: { getSessionId(): string; getSessionFile(): string | undefined };
};

type Pi = {
	on(event: string, handler: (event: unknown, ctx: Ctx) => unknown): void;
	sendUserMessage(content: string, options?: { deliverAs?: "steer" | "followUp" }): void;
	getSessionName?(): string | undefined;
};

type Answer = { ok: boolean; error?: string };

const PROTOCOL = 1;
/** The longest line read: teer's message is at most 32 KB, then escaped. */
const MAX_LINE = 256 * 1024;
/** A connection quiet for this long is closed. */
const IDLE_MS = 10_000;

/** pi's folder: $PI_CODING_AGENT_DIR (a leading ~ expanded), else ~/.pi/agent. */
function agentDir(): string {
	const env = process.env.PI_CODING_AGENT_DIR;
	if (!env) return path.join(os.homedir(), ".pi", "agent");
	if (env === "~") return os.homedir();
	if (env.startsWith("~/")) return path.join(os.homedir(), env.slice(2));
	return env;
}

/** Whether `dir` is (now) a real folder of ours that only we can enter. */
function privateDir(dir: string): boolean {
	try {
		try {
			fs.mkdirSync(dir, { mode: 0o700 });
		} catch (e) {
			if ((e as { code?: string }).code !== "EEXIST") return false;
		}
		const st = fs.lstatSync(dir);
		const uid = process.getuid?.();
		if (!st.isDirectory() || (uid !== undefined && st.uid !== uid)) return false;
		if ((st.mode & 0o077) !== 0) fs.chmodSync(dir, 0o700);
		return true;
	} catch {
		return false;
	}
}

/** Where this process listens; a socket path must stay short. */
function socketPath(): string | undefined {
	const uid = process.getuid?.() ?? 0;
	for (const base of [os.tmpdir(), "/tmp"]) {
		const dir = path.join(base, `teer-pi-${uid}`);
		const sock = path.join(dir, `${process.pid}.sock`);
		if (Buffer.byteLength(sock) < 100 && privateDir(dir)) return sock;
	}
	return undefined;
}

/** Remove the entries, and sockets, of pi processes that are gone. */
function sweep(inbox: string): void {
	for (const name of fs.readdirSync(inbox)) {
		const m = /^(\d+)\.json$/.exec(name);
		if (!m || Number(m[1]) === process.pid) continue;
		try {
			process.kill(Number(m[1]), 0);
			continue;
		} catch (e) {
			if ((e as { code?: string }).code !== "ESRCH") continue;
		}
		const file = path.join(inbox, name);
		try {
			const socket = JSON.parse(fs.readFileSync(file, "utf8")).socket;
			if (typeof socket === "string" && /\/teer-pi-\d+\/\d+\.sock$/.test(socket)) fs.unlinkSync(socket);
		} catch {}
		try {
			fs.unlinkSync(file);
		} catch {}
	}
}

function same(a: string, b: string): boolean {
	const x = Buffer.from(a);
	const y = Buffer.from(b);
	return x.length === y.length && crypto.timingSafeEqual(x, y);
}

export default function teerInbox(pi: Pi): void {
	let server: net.Server | undefined;
	let socket: string | undefined;
	let entry: string | undefined;
	let ctx: Ctx | undefined;
	let token = "";
	let sessionId = "";
	let startedAt = 0;

	function stop(): void {
		const s = server;
		server = undefined;
		try {
			s?.close();
		} catch {}
		for (const p of [socket, entry]) {
			if (!p) continue;
			try {
				fs.unlinkSync(p);
			} catch {}
		}
		socket = undefined;
		entry = undefined;
	}

	/** Write this session's entry, atomically, readable by us only. */
	function register(): void {
		if (!entry || !ctx) return;
		const data = {
			protocol: PROTOCOL,
			pid: process.pid,
			cwd: ctx.cwd,
			sessionId,
			sessionFile: ctx.sessionManager.getSessionFile() ?? null,
			socket,
			token,
			name: pi.getSessionName?.() ?? null,
			startedAt,
			updatedAt: Date.now(),
		};
		const tmp = `${entry}.${crypto.randomBytes(6).toString("hex")}.tmp`;
		fs.writeFileSync(tmp, JSON.stringify(data) + "\n", { mode: 0o600, flag: "wx" });
		fs.renameSync(tmp, entry);
	}

	/** One connection: the key, then one message; one answer, then close. */
	function serve(conn: net.Socket): void {
		let buf = "";
		let authed = false;
		let done = false;
		conn.setEncoding("utf8");
		conn.setTimeout(IDLE_MS, () => conn.destroy());
		conn.on("error", () => {});
		const answer = (a: Answer): void => {
			done = true;
			conn.end(JSON.stringify(a) + "\n");
		};
		const handle = (line: string): void => {
			let msg: { type?: unknown; token?: unknown; content?: unknown; session_id?: unknown } | null;
			try {
				msg = JSON.parse(line);
			} catch {
				return answer({ ok: false, error: "not JSON" });
			}
			if (!authed) {
				if (msg?.type === "auth" && typeof msg.token === "string" && token !== "" && same(msg.token, token)) {
					authed = true;
					return;
				}
				return answer({ ok: false, error: "auth" });
			}
			if (msg?.type !== "user" || typeof msg.content !== "string" || msg.content.trim() === "") {
				return answer({ ok: false, error: "not a message" });
			}
			if (msg.session_id !== sessionId) {
				return answer({ ok: false, error: "that session no longer runs in this pi" });
			}
			try {
				pi.sendUserMessage(msg.content, { deliverAs: "followUp" });
			} catch (e) {
				return answer({ ok: false, error: String((e as Error)?.message ?? e).slice(0, 200) });
			}
			try {
				if (ctx?.hasUI) ctx.ui?.notify("teer sent a message", "info");
			} catch {}
			answer({ ok: true });
		};
		conn.on("data", (chunk: string) => {
			if (done) return;
			buf += chunk;
			let i = buf.indexOf("\n");
			while (!done && i !== -1) {
				const line = buf.slice(0, i);
				buf = buf.slice(i + 1);
				handle(line);
				i = buf.indexOf("\n");
			}
			if (!done && buf.length > MAX_LINE) answer({ ok: false, error: "line too long" });
		});
	}

	pi.on("session_start", async (_event, c) => {
		stop();
		// Print and JSON runs end on their own: no one to send to.
		if (!c.hasUI) return;
		try {
			ctx = c;
			sessionId = c.sessionManager.getSessionId();
			token = crypto.randomBytes(16).toString("hex");
			startedAt = Date.now();
			const inbox = path.join(agentDir(), "teer-inbox");
			fs.mkdirSync(inbox, { recursive: true, mode: 0o700 });
			fs.chmodSync(inbox, 0o700);
			sweep(inbox);
			const sock = socketPath();
			if (!sock) throw new Error("no private folder for its socket");
			try {
				fs.unlinkSync(sock);
			} catch {}
			const srv = net.createServer(serve);
			await new Promise<void>((resolve, reject) => {
				srv.once("error", reject);
				srv.listen(sock, () => {
					srv.off("error", reject);
					resolve();
				});
			});
			srv.on("error", () => {});
			fs.chmodSync(sock, 0o600);
			server = srv;
			socket = sock;
			entry = path.join(inbox, `${process.pid}.json`);
			register();
		} catch (e) {
			stop();
			try {
				c.ui?.notify(`teer-inbox: teer can't reach this session (${(e as Error)?.message ?? e})`, "warning");
			} catch {}
		}
	});

	const touch = (): void => {
		try {
			register();
		} catch {}
	};
	pi.on("agent_end", touch);
	pi.on("session_info_changed", touch);
	pi.on("session_shutdown", () => stop());
}
