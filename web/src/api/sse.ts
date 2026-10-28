export interface SseEvent {
  event?: string;
  data: string;
}

/**
 * Reads a text/event-stream response body and yields complete SSE frames.
 * Handles CRLF, bare CR, comments and multi-line data without assuming chunk
 * alignment. Lines are consumed incrementally, so a chunk that ends mid-line
 * does not force a full-buffer rewrite.
 */
export async function* readSse(response: Response): AsyncGenerator<SseEvent> {
  if (!response.body) {
    throw new Error('response has no body');
  }

  const reader = response.body.getReader();
  const decoder = new TextDecoder();
  let buffer = '';
  let event: string | undefined;
  let data: string[] = [];
  let frames: SseEvent[] = [];

  function handleLine(line: string) {
    if (line === '') {
      if (data.length > 0) {
        frames.push({ event, data: data.join('\n') });
      }
      event = undefined;
      data = [];
      return;
    }
    if (line.startsWith(':')) {
      return;
    }
    if (line.startsWith('event:')) {
      event = line.slice(6).trim();
      return;
    }
    if (line.startsWith('data:')) {
      data.push(line.slice(5).replace(/^ /, ''));
    }
  }

  try {
    while (true) {
      const chunk = await reader.read();
      const finished = chunk.done;
      buffer += finished
        ? decoder.decode()
        : decoder.decode(chunk.value, { stream: true });

      let lineStart = 0;
      while (true) {
        // Find the next line terminator: LF, CRLF, or a bare CR.
        let lineEnd = -1;
        let skip = 1;
        for (let index = lineStart; index < buffer.length; index += 1) {
          const code = buffer.charCodeAt(index);
          if (code === 10) {
            lineEnd = index;
            break;
          }
          if (code === 13) {
            if (index + 1 >= buffer.length && !finished) {
              // A trailing CR may be the first half of CRLF in the next chunk.
              break;
            }
            lineEnd = index;
            skip =
              index + 1 < buffer.length && buffer.charCodeAt(index + 1) === 10
                ? 2
                : 1;
            break;
          }
        }

        if (lineEnd < 0) {
          break;
        }

        const line = buffer.slice(lineStart, lineEnd);
        lineStart = lineEnd + skip;
        handleLine(line);
      }

      if (lineStart > 0) {
        buffer = buffer.slice(lineStart);
      }

      if (finished) {
        if (buffer.length > 0) {
          handleLine(buffer);
          buffer = '';
        }
        // Flush a frame that the upstream ended without a trailing blank line.
        if (data.length > 0) {
          frames.push({ event, data: data.join('\n') });
          data = [];
        }
      }

      for (const frame of frames) {
        yield frame;
      }
      frames = [];

      if (finished) {
        break;
      }
    }
  } finally {
    reader.releaseLock();
  }
}
