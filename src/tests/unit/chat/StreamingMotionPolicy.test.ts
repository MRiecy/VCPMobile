import { describe, expect, it } from "vitest";
import messageRendererSource from "@/features/chat/MessageRenderer.vue?raw";
import chatBubbleSource from "@/features/chat/components/ChatBubble.vue?raw";
import groupStopSource from "@/features/chat/components/GroupStopAllButton.vue?raw";
import streamingTagSource from "@/features/chat/components/StreamingTag.vue?raw";
import thinkingIndicatorSource from "@/features/chat/components/ThinkingIndicator.vue?raw";
import thoughtBlockSource from "@/features/chat/blocks/ThoughtBlock.vue?raw";
import toolBlockSource from "@/features/chat/blocks/ToolBlock.vue?raw";
import messageBlocksCss from "@/assets/message-blocks.css?raw";

describe("streaming motion policy", () => {
  it("keeps long-lived reply status indicators static", () => {
    expect(chatBubbleSource).not.toContain("vcp-border-flow");
    expect(chatBubbleSource).not.toContain("will-change");
    expect(streamingTagSource).not.toContain("animation:");
    expect(groupStopSource).not.toContain("animate-pulse");
    expect(messageRendererSource).not.toContain("animate-pulse");
  });

  it("plays the initial thinking cue once without retaining a compositor hint", () => {
    expect(thinkingIndicatorSource).toContain(
      "animation: vcp-dot-pulse 900ms ease-in-out 1;",
    );
    expect(thinkingIndicatorSource).not.toContain("infinite");
    expect(thinkingIndicatorSource).not.toContain("will-change");
    expect(thinkingIndicatorSource).not.toContain("translate3d");
  });

  it("keeps tool and thought status chrome free of perpetual loaders", () => {
    expect(toolBlockSource).not.toContain("Loader2");
    expect(toolBlockSource).not.toContain("!block.is_complete");
    expect(toolBlockSource).not.toContain("custom-spin");
    expect(thoughtBlockSource).not.toContain("Loader2");
    expect(thoughtBlockSource).not.toContain("custom-spin");
    expect(messageBlocksCss).not.toContain("vcp-spin");
    expect(messageBlocksCss).not.toContain("custom-spin");
  });
});
