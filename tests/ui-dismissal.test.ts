import { expect, test } from "bun:test";
import { outsideBounds } from "../src/ui/dismissal";

test("native dialog padding and borders are content, not backdrop", () => {
  const bounds = { left: 100, top: 100, right: 500, bottom: 400 };
  for (const [clientX, clientY] of [
    [100, 100],
    [500, 400],
    [120, 120],
    [300, 250],
  ])
    expect(outsideBounds({ clientX, clientY }, bounds)).toBe(false);
  for (const [clientX, clientY] of [
    [99, 200],
    [501, 200],
    [200, 99],
    [200, 401],
  ])
    expect(outsideBounds({ clientX, clientY }, bounds)).toBe(true);
});
