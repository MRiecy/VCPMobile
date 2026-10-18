import { describe, expect, it } from "vitest";
import { mount } from "@vue/test-utils";
import DiaryReader from "@/features/diary/components/DiaryReader.vue";
import {
  highlightDiarySearchText,
  renderDiaryMarkdown,
} from "@/features/diary/diaryMarkdown";

describe("diary trusted body and text-only metadata boundary", () => {
  it("preserves trusted Markdown raw HTML only inside the body container", () => {
    const wrapper = mount(DiaryReader, {
      props: {
        document: {
          key: { folder: "<img src=x onerror=alert(1)>", file: "<b>name</b>.txt" },
          content: "# Heading\n\n<aside data-rich=\"kept\"><em>trusted html</em></aside>",
          contentHash: "hash",
        },
        loading: false,
        refreshing: false,
        error: null,
      },
    });

    expect(wrapper.find("article aside[data-rich='kept'] em").text()).toBe("trusted html");
    expect(wrapper.find("article h1").text()).toBe("Heading");
    expect(wrapper.find("header img").exists()).toBe(false);
    expect(wrapper.find("header b").exists()).toBe(false);
    expect(wrapper.find("header").text()).toContain("<b>name</b>.txt");
  });

  it("does not turn link-reference-like diary lines into hidden definitions", () => {
    const html = renderDiaryMarkdown("[AI]: visible\n\n<section>raw</section>");
    expect(html).toContain("[AI]: visible");
    expect(html).toContain("<section>raw</section>");
  });

  it("highlights literal text matches without rewriting markup or code", () => {
    const html = highlightDiarySearchText(
      '<p data-kind="kept">Alpha alpha</p><pre><code>alpha</code></pre>',
      "alpha",
    );
    const container = document.createElement("div");
    container.innerHTML = html;

    expect(container.querySelector("p")?.dataset.kind).toBe("kept");
    expect(container.querySelectorAll("p mark.diary-search-mark")).toHaveLength(2);
    expect(container.querySelector("p")?.textContent).toBe("Alpha alpha");
    expect(container.querySelector("code mark")).toBeNull();
    expect(container.querySelector("code")?.textContent).toBe("alpha");
  });

  it("keeps original UTF-16 match offsets when Unicode case folding expands", () => {
    const container = document.createElement("div");
    container.innerHTML = highlightDiarySearchText("<p>İx X</p>", "x");

    expect(container.querySelector("p")?.textContent).toBe("İx X");
    expect([...container.querySelectorAll("mark")].map((item) => item.textContent))
      .toEqual(["x", "X"]);
  });
});
