import { describe, expect, it } from "vitest";
import {
  renderMessageRawHtml,
  shouldRenderMessageHtml,
} from "../../core/utils/safeMessageHtml";

describe("safeMessageHtml", () => {
  it("将模型伪标签按原文显示", () => {
    expect(renderMessageRawHtml("<reason>")).toBe("&lt;reason&gt;");
    expect(renderMessageRawHtml("</reason>")).toBe("&lt;/reason&gt;");
  });

  it("保留项目支持的富文本标签", () => {
    expect(shouldRenderMessageHtml('<span class="tone">')).toBe(true);
    expect(renderMessageRawHtml("</span>")).toBe("</span>");
  });

  it("保留内部消息分隔注释", () => {
    expect(renderMessageRawHtml("<!--brk-->")).toBe("<!--brk-->");
  });
});
