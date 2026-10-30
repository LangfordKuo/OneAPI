/** Human-readable text for anything thrown by a mutation or fetch. */
export function errorMessage(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}
