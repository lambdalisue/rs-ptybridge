#!/usr/bin/env -S deno run --allow-run --allow-env
// Minimal MessagePack consumer (Deno): run a command under
// `ptybridge --format msgpack` and repaint its screen as text. Display-only —
// it shows how to decode the self-delimiting MessagePack event stream and place
// cells per the protocol (honoring `col` and the `grid_scroll` rectangle). The
// JSONL host in passthrough.ts is the full bidirectional, colored version.
//
//   deno run --allow-run --allow-env examples/msgpack_render.ts -- top
//
// Set PTYBRIDGE_BIN to the daemon binary (defaults to `ptybridge` on PATH).
import { decodeMultiStream } from "@msgpack/msgpack";

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

async function main() {
  const command = Deno.args[0] === "--" ? Deno.args.slice(1) : Deno.args;
  if (command.length === 0) {
    console.error("usage: msgpack_render.ts -- <command> [args...]");
    Deno.exit(2);
  }

  const { columns: cols, rows } = Deno.consoleSize();
  const bin = Deno.env.get("PTYBRIDGE_BIN") ?? "ptybridge";
  const child = new Deno.Command(bin, {
    args: [
      "--format",
      "msgpack",
      "--cols",
      String(cols),
      "--rows",
      String(rows),
      "--",
      ...command,
    ],
    // Keep stdin piped and open so the daemon's control reader blocks instead of
    // hitting EOF; the session ends when the child exits.
    stdin: "piped",
    stdout: "piped",
    stderr: "inherit",
  }).spawn();

  await write("\x1b[?1049h\x1b[?7l\x1b[2J"); // alt screen, no autowrap, clear
  let grid: string[][] = [];
  let gridCols = 0;
  let code = 0;

  for await (const raw of decodeMultiStream(child.stdout)) {
    const msg = raw as Msg;
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
        const line = grid[msg.row ?? 0];
        if (line && msg.cells) {
          let c = msg.col ?? 0;
          for (const cell of msg.cells) {
            const repeat = cell[2] ?? 1;
            for (let i = 0; i < repeat; i++) {
              if (c < line.length) line[c] = cell[0];
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
