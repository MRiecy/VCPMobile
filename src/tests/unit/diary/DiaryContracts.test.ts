import { describe, expect, it } from "vitest";
import { mount } from "@vue/test-utils";
import DiaryReader from "@/features/diary/components/DiaryReader.vue";
import DiaryNoteList from "@/features/diary/components/DiaryNoteList.vue";
import {
  highlightDiarySearchText,
  renderDiaryMarkdown,
} from "@/features/diary/diaryMarkdown";
import {
  diaryFolderCategory,
  isValidDiaryFileName,
  parseDiaryError,
  parseDiaryFileName,
} from "@/features/diary/types";

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

describe("diary pure presentation helpers", () => {
  it("parses structured names and falls back for arbitrary names", () => {
    expect(parseDiaryFileName("2026-08-12-19_30_00-新的标题.txt")).toMatchObject({
      title: "新的标题",
      date: "2026-08-12",
      time: "19:30:00",
      extension: "TXT",
      structured: true,
    });
    expect(parseDiaryFileName("任意 文件名.v1.md")).toMatchObject({
      title: "任意 文件名.v1.md",
      extension: "MD",
      structured: false,
    });
    expect(parseDiaryFileName("2026-08-12-19_30_00.txt")).toMatchObject({
      title: "DailyNote",
      date: "2026-08-12",
      time: "19:30:00",
      extension: "TXT",
      structured: true,
    });
  });

  it("keeps stable folder grouping and fail-closed rename validation", () => {
    expect(diaryFolderCategory("项目设计簇")).toBe("cluster");
    expect(diaryFolderCategory("Nova 的知识")).toBe("diary");
    expect(isValidDiaryFileName("safe name.txt")).toBe(true);
    expect(isValidDiaryFileName("../unsafe.txt")).toBe(false);
    expect(isValidDiaryFileName("a\\b.txt")).toBe(false);
  });

  it("parses stable error codes without interpreting natural-language text", () => {
    expect(parseDiaryError("DIARY_CONFLICT: changed")).toEqual({
      code: "DIARY_CONFLICT",
      message: "changed",
    });
    expect(parseDiaryError(new Error("network"))).toEqual({
      code: "DIARY_UNKNOWN",
      message: "network",
    });
  });
});

describe("diary mobile presentation structure", () => {
  it("renders VCP-style structured memo rows without exposing timestamp filenames as titles", async () => {
    const wrapper = mount(DiaryNoteList, {
      props: {
        notes: [{
          folder: "Aesthetic",
          file: "2026-06-12-23_37_44.txt",
          preview: "Nova 与主人共同淬炼的流性美学灵光",
          lastModified: "2026-06-12T23:37:44+08:00",
        }],
        loading: false,
        searchMode: "none",
        selectionMode: false,
        selectedIds: [],
      },
      global: {
        directives: { longpress: () => undefined },
      },
    });

    await wrapper.vm.$nextTick();

    const row = wrapper.get('[data-diary-role="note-row"]');
    expect(row.text()).toContain("TXT");
    expect(row.text()).toContain("DailyNote");
    expect(row.text()).toContain("2026-06-12 23:37");
    expect(row.text()).toContain("流性美学灵光");
  });
});
