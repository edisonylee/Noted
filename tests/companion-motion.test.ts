import { describe, expect, test } from "bun:test";
import { containPet, edgePull, panelNearPet, rememberPet, restorePet } from "../src/companionMotion";
import { parseCompanion } from "../src/companionStore";

describe("companion movement", () => {
  const viewport = { width: 1000, height: 700 };
  test("a free position survives resizing and remains fully reachable", () => {
    const position = rememberPet({ x: 416, y: 320 }, viewport, 84);
    const restored = restorePet(position, "right", viewport, 84);
    expect(restored.x).toBeCloseTo(416);
    expect(restored.y).toBeCloseTo(320);
    const smaller = restorePet(position, "right", { width: 400, height: 300 }, 104);
    expect(smaller.x).toBeGreaterThanOrEqual(8);
    expect(smaller.x + 104).toBeLessThanOrEqual(392);
    expect(smaller.y + 104).toBeLessThanOrEqual(292);
    expect(containPet({ x: -200, y: 1500 }, viewport, 84)).toEqual({ x: 8, y: 608 });
  });
  test("edge resistance fills only during an intentional outward pull", () => {
    const start = { x: 600, y: 350 };
    expect(edgePull({ x: 950, y: 350 }, start, viewport)).toEqual({ edge: "right", progress: 30 / 72, ready: false });
    expect(edgePull({ x: 992, y: 350 }, start, viewport).ready).toBe(true);
    expect(edgePull({ x: 970, y: 350 }, { x: 990, y: 350 }, viewport).progress).toBe(0);
    expect(edgePull({ x: 998, y: 350 }, { x: 994, y: 350 }, viewport).ready).toBe(false);
    for (const pointer of [{ x: 5, y: 350 }, { x: 500, y: 5 }, { x: 500, y: 695 }]) {
      expect(edgePull(pointer, { x: 500, y: 350 }, viewport).ready).toBe(true);
    }
  });
  test("every corner keeps chat inside the window", () => {
    for (const point of [{ x: 8, y: 8 }, { x: 908, y: 8 }, { x: 8, y: 608 }, { x: 908, y: 608 }]) {
      const panel = panelNearPet(point, viewport, 84, 480, 680);
      expect(panel.left).toBeGreaterThanOrEqual(12);
      expect(panel.left + 480).toBeLessThanOrEqual(988);
      expect(panel.top).toBeGreaterThanOrEqual(12);
      expect(panel.top + Math.min(680, panel.maxHeight)).toBeLessThanOrEqual(688);
    }
  });
  test("legacy preferences get a home position; corrupt coordinates cannot strand a pet", () => {
    expect(parseCompanion('{"petId":"wisp"}').position).toBeNull();
    expect(parseCompanion('{"position":{"x":"1","y":null}}').position).toBeNull();
    expect(parseCompanion('{"position":{"x":-2,"y":9}}').position).toEqual({ x: 0, y: 1 });
  });
});
