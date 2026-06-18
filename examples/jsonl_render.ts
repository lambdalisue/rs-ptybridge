#!/usr/bin/env -S deno run --allow-run --allow-env
// Minimal JSONL consumer (Deno): run a command under `ptybridge` (the default
// JSONL format) and repaint its screen as text. Display-only — it shows how to
// decode the newline-delimited event stream and place cells per the protocol
// (honoring `col` and the `grid_scroll` rectangle). The MessagePack twin is
// msgpack_render.ts; the full bidirectional, colored host is passthrough.ts.
//
//   deno run --allow-run --allow-env examples/jsonl_render.ts -- top
//
// Set PTYBRIDGE_BIN to the bridge binary (defaults to `ptybridge` on PATH).

type Cell = [string, (number | null)?, number?];

/** The fields of the events this renderer reads (others are ignored). */
interface Msg {
  t: string;
  cols?: number;
  rows?: number;
  row?: number;
  col?: number;
  top?: number;
  bot?: number;
  left?: number;
  right?: number;
  cells?: Cell[];
  code?: number | null;
}

const enc = new TextEncoder();
const write = (s: string) => Deno.stdout.write(enc.encode(s));

/** A blank `rows`×`cols` grid; one grapheme per cell, an empty cell is the
 * trailing half of a wide character. */
function blankGrid(cols: number, rows: number): string[][] {
  return Array.from(
    { length: rows },
    () => Array.from({ length: cols }, () => " "),
  );
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
    console.error("usage: jsonl_render.ts -- <command> [args...]");
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

  await write("\x1b[?1049h\x1b[?7l\x1b[2J"); // alt screen, no autowrap, clear
  let grid: string[][] = [];
  let gridCols = 0;
  let code = 0;

  for await (const line of lines(child.stdout)) {
    if (line.trim() === "") continue;
    let msg: Msg;
    try {
      msg = JSON.parse(line) as Msg;
    } catch {
      continue; // ignore unknown / malformed lines (forward compatible)
    }
    switch (msg.t) {
      case "hello":
      case "grid_resize":
        gridCols = msg.cols ?? 0;
        grid = blankGrid(gridCols, msg.rows ?? 0);
        break;
      case "grid_clear":
        for (const row of grid) row.fill(" ");
        break;
      case "grid_scroll": {
        const k = msg.rows ?? 0;
        const top = msg.top ?? 0;
        const bot = msg.bot ?? grid.length;
        const left = msg.left ?? 0;
        const right = msg.right ?? gridCols;
        if (k > 0) {
          for (let r = top; r < bot; r++) {
            for (let c = left; c < right; c++) {
              grid[r][c] = r + k < bot ? grid[r + k][c] : " ";
            }
          }
        }
        break;
      }
      case "grid_line": {
        const lineRow = grid[msg.row ?? 0];
        if (lineRow && msg.cells) {
          let c = msg.col ?? 0;
          for (const cell of msg.cells) {
            const repeat = cell[2] ?? 1;
            for (let i = 0; i < repeat; i++) {
              if (c < lineRow.length) lineRow[c] = cell[0];
              c++;
            }
          }
        }
        break;
      }
      case "flush": {
        let frame = "\x1b[H";
        for (const row of grid) frame += `\x1b[K${row.join("")}\r\n`;
        await write(frame);
        break;
      }
      case "child_exit":
        code = msg.code ?? 0;
        break;
    }
    if (msg.t === "child_exit") break;
  }

  await write("\x1b[?7h\x1b[?1049l");
  try {
    await child.stdin.close();
  } catch { /* already gone */ }
  await child.status;
  Deno.exit(code);
}

if (import.meta.main) {
  await main();
}
