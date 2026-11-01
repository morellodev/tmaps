export function describe(error: unknown) {
  return error instanceof Error ? error.message : String(error);
}
