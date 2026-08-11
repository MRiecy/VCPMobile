import { describe, expect, it } from "vitest";
import { LatestIntentOwner } from "@/core/utils/latestIntentOwner";

describe("LatestIntentOwner", () => {
  it("rejects a late completion from an older share intent", async () => {
    const owner = new LatestIntentOwner();
    const committed: string[] = [];
    let finishFirst!: () => void;
    const firstReady = new Promise<void>((resolve) => {
      finishFirst = resolve;
    });

    const first = owner.begin();
    const lateFirst = (async () => {
      await firstReady;
      if (owner.isCurrent(first)) committed.push("A");
    })();

    const second = owner.begin();
    if (owner.isCurrent(second)) committed.push("B");
    finishFirst();
    await lateFirst;

    expect(committed).toEqual(["B"]);
    expect(owner.isCurrent(first)).toBe(false);
  });

  it("only clears the owner that still holds the intent", () => {
    const owner = new LatestIntentOwner();
    const first = owner.begin();
    const second = owner.begin();

    owner.clear(first);
    expect(owner.isCurrent(second)).toBe(true);
    owner.clear(second);
    expect(owner.isCurrent(second)).toBe(false);
  });
});
