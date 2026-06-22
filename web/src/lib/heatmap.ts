export function normalizeBand(values: string[]): number[] {
  const nums = values.map((v) => Number(v)); // relative use → f64 precision loss tolerable
  const max = Math.max(0, ...nums);
  return nums.map((v) => (max === 0 ? 0 : v / max));
}
export function pageStrike(i: number, n: number, min: number, max: number): number {
  if (n <= 1) return min;
  return min + (i / (n - 1)) * (max - min);
}
