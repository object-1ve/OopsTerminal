/**
 * 把 xterm 的原始输入流转成可提交的命令行。
 *
 * 终端输入会混入方向键、F 键、鼠标事件等转义序列，扫描时跳过它们；
 * 退格和 Delete 修改当前缓冲，回车把整行作为一条命令提交。
 */

/** ESC 序列在 input[start] 处的字符长度；start 处不是 ESC 时返回 0。 */
export function escapeSequenceLength(input: string, start: number): number {
  if (input[start] !== "\u001b" || start + 1 >= input.length) {
    return input[start] === "\u001b" ? 1 : 0;
  }

  const second = input[start + 1];
  const end = input.length;

  if (second === "[") {
    let i = start + 2;
    while (i < end) {
      const code = input.charCodeAt(i);
      i += 1;
      if (code >= 0x40 && code <= 0x7e) break;
    }
    return i - start;
  }

  if (second === "]") {
    let i = start + 2;
    while (i < end) {
      if (input.charCodeAt(i) === 0x07) {
        i += 1;
        break;
      }
      if (input[i] === "\u001b" && input[i + 1] === "\\") {
        i += 2;
        break;
      }
      i += 1;
    }
    return i - start;
  }

  if (second === "O" && start + 2 < end) return 3;
  return 2;
}

/** 从当前缓冲和输入块中提取提交行；空行不会返回。 */
export function scanInputChunk(
  buf: string,
  chunk: string,
): { buf: string; lines: string[] } {
  let b = buf;
  const lines: string[] = [];
  let i = 0;

  while (i < chunk.length) {
    const c = chunk[i];

    if (c === "\r" || c === "\n") {
      if (b.trim().length > 0) lines.push(b);
      b = "";
      i += 1;
      continue;
    }

    if (c === "\u001b") {
      const len = escapeSequenceLength(chunk, i);
      i += len > 0 ? len : 1;
      continue;
    }

    const code = c.charCodeAt(0);
    if (code === 0x08 || code === 0x7f) {
      b = Array.from(b).slice(0, -1).join("");
      i += 1;
      continue;
    }

    if (code === 0x03 || code === 0x1a) {
      b = "";
      i += 1;
      continue;
    }

    if (code >= 0x20 || c === "\t") {
      b += c;
    }
    i += 1;
  }

  return { buf: b, lines };
}
