#!/usr/bin/env -S deno run --allow-run --allow-env
// Transparent reference host (Deno). Run a command under `ptybridge` but
// reconstruct its screen back into ANSI on the real terminal, so
//
//   deno run --allow-run --allow-env examples/passthrough.ts -- claude
//
// looks and behaves as if you ran `claude` directly: input is forwarded and the
// emulated screen is repainted with colors, cursor, and title. It is the inverse
// of the bridge — where `ptybridge` turns ANSI into screen-state JSONL, this
// turns the JSONL screen state back into ANSI.
//
// This is a reference *host*: it holds the grid the protocol guarantees and
// places cells by element count, doing no width computation of its own. It
// renders onto the alternate screen and repaints on every `flush`, which suits
// full-screen TUIs (the common case); a primary-screen scrolling program would
// want a smarter strategy.
//
// Set PTYBRIDGE_BIN to the bridge binary (defaults to `ptybridge` on PATH).

type Attrs = {
  fg?: number | null;
  bg?: number | null;
  bold?: boolean;
  italic?: boolean;
  underline?: boolean;
  undercurl?: boolean;
  reverse?: boolean;
  strikethrough?: boolean;
  dim?: boolean;
  blink?: boolean;
};

type Cell = [string, (number | null)?, number?];

const enc = new TextEncoder();

/** Translate a highlight's attributes into an SGR sequence (truecolor). */
function sgr(a: Attrs | undefined): string {
  if (!a) return "\x1b[0m";
  const p: string[] = ["0"]; // reset, then layer attributes on
  if (a.bold) p.push("1");
  if (a.dim) p.push("2");
  if (a.italic) p.push("3");
  if (a.underline || a.undercurl) p.push("4");
  if (a.blink) p.push("5");
  if (a.reverse) p.push("7");
  if (a.strikethrough) p.push("9");
  const rgb = (c: number) =>
    `${(c >> 16) & 0xff};${(c >> 8) & 0xff};${c & 0xff}`;
  p.push(a.fg != null ? `38;2;${rgb(a.fg)}` : "39");
  p.push(a.bg != null ? `48;2;${rgb(a.bg)}` : "49");
  return `\x1b[${p.join(";")}m`;
}

/** Reconstructed screen: the grid the host renders, mirroring the bridge. */
class Host {
  cols = 0;
  rows = 0;
  grid: { text: string; hl: number }[][] = [];
  hl = new Map<number, Attrs>();
  cursorRow = 0;
  cursorCol = 0;
  cursorVisible = true;
  cursorShape = 2;
  title: string | null = null;
  bell = false;

  resize(cols: number, rows: number) {
    this.cols = cols;
    this.rows = rows;
    this.grid = Array.from(
      { length: rows },
      () => Array.from({ length: cols }, () => ({ text: " ", hl: 0 })),
    );
  }

  // deno-lint-ignore no-explicit-any
  apply(ev: any) {
    switch (ev.t) {
      case "hello":
      case "grid_resize":
        this.resize(ev.cols, ev.rows);
        break;
      case "hl_attr": {
        const { t: _t, id, ...attrs } = ev;
        this.hl.set(id, attrs as Attrs);
        break;
      }
      case "grid_clear":
        for (const row of this.grid) {
          for (const col of row) {
            col.text = " ";
            col.hl = 0;
          }
        }
        break;
      case "grid_scroll": {
        const k = ev.rows as number;
        if (k > 0) {
          const top = ev.top as number;
          const bot = ev.bot as number;
          for (let r = top; r < bot; r++) {
            this.grid[r] = r + k < bot
              ? this.grid[r + k]
              : Array.from({ length: this.cols }, () => ({ text: " ", hl: 0 }));
          }
        }
        break;
      }
      case "scrollback_push":
        // This host is a live-screen mirror: it repaints the visible grid onto
        // the real terminal, which keeps its own scrollback. Committed lines are
        // discarded here; a host backed by an editor buffer would append them.
        break;
      case "grid_line": {
        const line = this.grid[ev.row];
        if (!line) break;
        let c = ev.col as number;
        let hl = 0;
        for (const cell of ev.cells as Cell[]) {
          if (cell.length > 1 && cell[1] != null) hl = cell[1] as number;
          const repeat = cell.length > 2 ? (cell[2] as number) : 1;
          for (let i = 0; i < repeat; i++) {
            if (line[c]) line[c] = { text: cell[0], hl };
            c++;
          }
        }
        break;
      }
      case "cursor":
        this.cursorRow = ev.row;
        this.cursorCol = ev.col;
        this.cursorVisible = ev.visible;
        this.cursorShape = ev.shape === "bar"
          ? 6
          : ev.shape === "underline"
          ? 4
          : 2;
        break;
      case "title":
        this.title = ev.text;
        break;
      case "bell":
        this.bell = true;
        break;
    }
  }

  /** Repaint the whole grid onto the alternate screen, place the cursor, and
   * flush any title/bell accumulated this frame. */
  render(): string {
    let out = "";
    if (this.title != null) {
      out += `\x1b]0;${this.title}\x07`;
      this.title = null;
    }
    out += "\x1b[?25l"; // hide cursor while painting to avoid flicker
    for (let r = 0; r < this.grid.length; r++) {
      out += `\x1b[${r + 1};1H\x1b[K`;
      let current = -1;
      for (const col of this.grid[r]) {
        if (col.text === "") continue; // trailing half of a wide cell
        if (col.hl !== current) {
          out += sgr(this.hl.get(col.hl));
          current = col.hl;
        }
        out += col.text;
      }
    }
    out += "\x1b[0m";
    out += `\x1b[${this.cursorShape} q`;
    out += `\x1b[${this.cursorRow + 1};${this.cursorCol + 1}H`;
    if (this.cursorVisible) out += "\x1b[?25h";
    if (this.bell) {
      out += "\x07";
      this.bell = false;
    }
    return out;
  }
}

function base64(bytes: Uint8Array): string {
  let bin = "";
  for (const b of bytes) bin += String.fromCharCode(b);
  return btoa(bin);
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
  // `deno run script.ts -- claude` passes the `--` through verbatim; drop it.
  const command = Deno.args[0] === "--" ? Deno.args.slice(1) : Deno.args;
  if (command.length === 0) {
    console.error("usage: passthrough.ts -- <command> [args...]");
    Deno.exit(2);
  }

  const { columns: cols, rows } = Deno.consoleSize();
  const bin = Deno.env.get("PTYBRIDGE_BIN") ?? "ptybridge";
  const child = new Deno.Command(bin, {
    args: ["--cols", String(cols), "--rows", String(rows), "--", ...command],
    stdin: "piped",
    stdout: "piped",
    stderr: "inherit",
  }).spawn();

  const writer = child.stdin.getWriter();
  const send = (obj: unknown) =>
    writer.write(enc.encode(JSON.stringify(obj) + "\n"));

  // Raw mode + alternate screen for the duration of the session.
  Deno.stdin.setRaw(true);
  await Deno.stdout.write(enc.encode("\x1b[?1049h\x1b[?7l\x1b[2J"));

  const onResize = () => {
    const size = Deno.consoleSize();
    send({ t: "resize", cols: size.columns, rows: size.rows }).catch(() => {});
  };
  Deno.addSignalListener("SIGWINCH", onResize);

  // Forward the real terminal's input to the bridge as base64 (binary-safe).
  const forwardInput = (async () => {
    const buf = new Uint8Array(4096);
    while (true) {
      const n = await Deno.stdin.read(buf);
      if (n === null) break;
      try {
        await send({
          t: "input",
          enc: "base64",
          data: base64(buf.subarray(0, n)),
        });
      } catch {
        break;
      }
    }
  })();

  const host = new Host();
  let code = 0;
  for await (const line of lines(child.stdout)) {
    if (line.trim() === "") continue;
    // deno-lint-ignore no-explicit-any
    let ev: any;
    try {
      ev = JSON.parse(line);
    } catch {
      continue; // ignore unknown / malformed lines (forward compatible)
    }
    if (ev.t === "child_exit") {
      code = ev.code ?? 0;
      break;
    } else if (ev.t === "flush") {
      await Deno.stdout.write(enc.encode(host.render()));
    } else {
      host.apply(ev);
    }
  }

  // Restore the terminal on every exit path.
  Deno.removeSignalListener("SIGWINCH", onResize);
  await Deno.stdout.write(enc.encode("\x1b[?7h\x1b[?1049l"));
  Deno.stdin.setRaw(false);
  try {
    await writer.close();
  } catch { /* bridge already gone */ }
  await child.status;
  forwardInput.catch(() => {});
  Deno.exit(code);
}

if (import.meta.main) {
  await main();
}
