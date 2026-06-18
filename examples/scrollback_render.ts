#!/usr/bin/env -S deno run --allow-run --allow-env
// Scrollback-aware consumer (Deno): run a command under `ptybridge` and keep an
// append-only buffer — committed scrollback above a live region — exactly as a
// host rendering into a normal editor buffer (Vim/Neovim) would. On exit it
// dumps the whole transcript, so lines that scrolled off the screen survive.
//
//   deno run --allow-run --allow-env examples/scrollback_render.ts -- bash -c 'seq 1 100'
//
// Compare with jsonl_render.ts, which keeps only the visible grid: there a
// 100-line `seq` shows just the last screenful, here all 100 lines survive.

type Cell = [string, (number | null)?, number?];

interface Msg {
  t: string;
  rows?: number;
  row?: number;
  cells?: Cell[];
  lines?: Cell[][];
  code?: number | null;
}

/** Expand a row's cells into plain text (one grapheme per cell, runs expanded),
 * trimming trailing blanks so the dump reads like a log. */
function rowText(cells: Cell[]): string {
  let text = "";
  for (const cell of cells) {
    const repeat = cell[2] ?? 1;
    for (let i = 0; i < repeat; i++) text += cell[0];
  }
  return text.replace(/\s+$/, "");
}

/** The terminal size, falling back to 80×24 when stdout is not a tty (e.g. the
 * output is piped to a file). */
function terminalSize(): { columns: number; rows: number } {
  try {
    return Deno.consoleSize();
  } catch {
    return { columns: 80, rows: 24 };
  }
}

/** Yield `\n`-delimited lines from a byte stream. */
async function* lines(stream: ReadableStream<Uint8Array>) {
  const reader = stream.getReader();
  const dec = new TextDecoder();
  let buf = "";
  while (true) {
    const { value, done } = await reader.read();
    if (done) break;
    buf += dec.decode(value, { stream: true });
    let idx: number;
    while ((idx = buf.indexOf("\n")) >= 0) {
      yield buf.slice(0, idx);
      buf = buf.slice(idx + 1);
    }
  }
  if (buf) yield buf;
}

async function main() {
  const command = Deno.args[0] === "--" ? Deno.args.slice(1) : Deno.args;
  if (command.length === 0) {
    console.error("usage: scrollback_render.ts -- <command> [args...]");
    Deno.exit(2);
  }

  const { columns: cols, rows } = terminalSize();
  const bin = Deno.env.get("PTYBRIDGE_BIN") ?? "ptybridge";
  const child = new Deno.Command(bin, {
    args: ["--cols", String(cols), "--rows", String(rows), "--", ...command],
    // Keep stdin piped and open so the bridge's control reader blocks instead of
    // hitting EOF; the session ends when the child exits.
    stdin: "piped",
    stdout: "piped",
    stderr: "inherit",
  }).spawn();

  // The buffer the host owns: committed scrollback (grown only by
  // `scrollback_push`) above a fixed live region (the current visible grid).
  const committed: string[] = [];
  let live: string[] = [];
  let code = 0;

  for await (const raw of lines(child.stdout)) {
    if (raw.trim() === "") continue;
    let msg: Msg;
    try {
      msg = JSON.parse(raw) as Msg;
    } catch {
      continue; // ignore unknown / malformed lines (forward compatible)
    }
    switch (msg.t) {
      case "hello":
      case "grid_resize":
        live = Array.from({ length: msg.rows ?? 0 }, () => "");
        break;
      // Lines that left the top of the screen: append them, never rewrite.
      case "scrollback_push":
        for (const cells of msg.lines ?? []) committed.push(rowText(cells));
        break;
      case "grid_clear":
        live = live.map(() => "");
        break;
      // A live-region shift: move rows up, leaving the bottom stale (the next
      // grid_line repaints it). Scrollback is untouched.
      case "grid_scroll": {
        const k = msg.rows ?? 0;
        if (k > 0) {
          for (let r = 0; r < live.length; r++) {
            if (r + k < live.length) live[r] = live[r + k];
          }
        }
        break;
      }
      case "grid_line":
        if (msg.row != null && msg.cells && live[msg.row] != null) {
          live[msg.row] = rowText(msg.cells);
        }
        break;
      case "child_exit":
        code = msg.code ?? 0;
        break;
    }
    if (msg.t === "child_exit") break;
  }

  try {
    await child.stdin.close();
  } catch { /* already gone */ }
  await child.status;

  // Dump the full transcript: scrollback first, then the live region. Without
  // scrollback the committed lines would be lost the moment they scrolled off.
  const out = [...committed, ...live].join("\n");
  await Deno.stdout.write(new TextEncoder().encode(out + "\n"));
  console.error(
    `[scrollback_render] ${committed.length} scrollback + ${live.length} live lines`,
  );
  Deno.exit(code);
}

if (import.meta.main) {
  await main();
}
