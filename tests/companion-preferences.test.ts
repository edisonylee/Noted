import { describe, expect, test } from "bun:test";
import { parseCompanion } from "../src/companionStore";

describe("companion preferences", () => {
  test("recovers from malformed or missing local preferences", () => {
    for (const value of [null, "{", "null", "[]", '"text"']) {
      expect(parseCompanion(value).petId).toBe("nib");
    }
  });
  test("rejects external and executable image URLs", () => {
    const customPets = ["https://example.com/pet.png", "javascript:alert(1)", "data:image/svg+xml,<svg/>"]
      .map((image, index) => ({ id: `custom-${index}`, name: "Pet", image }));
    const parsed = parseCompanion(JSON.stringify({ petId: "custom-0", customPets }));
    expect(parsed.customPets).toEqual([]);
    expect(parsed.petId).toBe("nib");
  });
  test("retains valid custom artwork and preferences", () => {
    const pet = { id: "custom-test", name: "Dot", image: "data:image/png;base64,YQ==" };
    const parsed = parseCompanion(JSON.stringify({ petId: pet.id, name: "  Dot  ", size: "large", side: "left", motion: false, customPets: [pet] }));
    expect(parsed).toEqual({ petId: pet.id, name: "Dot", size: "large", side: "left", motion: false, position: null, customPets: [pet] });
  });
  test("bounds storage and repairs invalid selections", () => {
    const customPets = Array.from({ length: 12 }, (_, i) => ({ id: `custom-${i}`, name: "Pet", image: "data:image/png;base64,YQ==" }));
    const parsed = parseCompanion(JSON.stringify({ petId: "missing", name: "a".repeat(100), size: "huge", side: "top", customPets }));
    expect(parsed.customPets).toHaveLength(8);
    expect(parsed.name).toHaveLength(30);
    expect(parsed.size).toBe("medium");
    expect(parsed.side).toBe("right");
  });
});
