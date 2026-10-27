export interface SseEvent {
  event?: string;
  data: string;
}

/**
 * Reads a text/event-stream response body and yields complete SSE frames.
 * Handles CRLF, comments and multi-line data without assuming chunk alignment.
 */
export async function* readSse(response: Response): AsyncGenerator<SseEvent> {
  if (!response.body) {
    throw new Error('response has no body');
  }

  const reader = response.body.getReader();
  const decoder = new TextDecoder();
  let buffer = '';

  try {
    while (true) {
      const { done, value } = await reader.read();
      if (done) {
        break;
      }
      buffer += decoder.decode(value, { stream: true });
      buffer = buffer.replace(/\r\n/g, '\n');

      let boundary = buffer.indexOf('\n\n');
      while (boundary >= 0) {
        const frame = buffer.slice(0, boundary);
        buffer = buffer.slice(boundary + 2);

        let event: string | undefined;
        const data: string[] = [];
        for (const line of frame.split('\n')) {
          if (line.startsWith(':')) {
            continue;
          }
          if (line.startsWith('event:')) {
            event = line.slice(6).trim();
          } else if (line.startsWith('data:')) {
            data.push(line.slice(5).replace(/^ /, ''));
          }
        }
        if (data.length > 0) {
          yield { event, data: data.join('\n') };
        }

        boundary = buffer.indexOf('\n\n');
      }
    }
  } finally {
    reader.releaseLock();
  }
}
