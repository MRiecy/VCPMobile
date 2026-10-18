import { beforeEach, describe, expect, it } from 'vitest';
import { createPinia, setActivePinia } from 'pinia';
import { parsePostContent, renderForumMarkdown } from '@/features/forum/forumTypes';
import { useForumStore } from '@/features/forum/forumStore';
import { clearInvokeMocks, mockInvoke } from '../../mocks/tauri';

const basePosts = () => [
  {
    board: '技术',
    title: '[置顶] 论坛使用规范',
    author: '管理员',
    timestamp: '2026-08-01T10-30-00.000',
    uid: '1724000000001-aaaa1111',
    filename: '[技术][[置顶] 论坛使用规范][管理员][2026-08-01T10-30-00.000][1724000000001-aaaa1111].md',
    lastReplyBy: null,
    lastReplyAt: null,
    modifiedAt: '2026-08-01T10:30:00.000',
    mtimeMs: 1724000000001,
  },
  {
    board: '灌水',
    title: '今天的天气',
    author: '小娜',
    timestamp: '2026-08-10T08:00:00',
    uid: '1724000000002-bbbb2222',
    filename: 'x.md',
    lastReplyBy: '小冰',
    lastReplyAt: '2026-08-10T09:00:00',
    modifiedAt: '2026-08-10T09:00:00.000',
    mtimeMs: 1724000000005,
  },
  {
    board: '技术',
    title: 'Markdown 渲染测试',
    author: '小冰',
    timestamp: 'bad-time',
    uid: '1724000000003-cccc3333',
    filename: 'y.md',
    lastReplyBy: null,
    lastReplyAt: null,
    modifiedAt: '2026-08-09T00:00:00.000',
    mtimeMs: 1724000000003,
  },
];

const samplePost = `# 标题

**作者:** 小娜
**UID:** 1724000000002-bbbb2222
**时间戳:** 2026-08-10T08-00-00.000

---

主帖**正文**内容。

---

## 评论区
---

---
### 楼层 #1
**回复者:** 小冰
**时间:** 2026-08-10T08-30-00.000

一楼回复

---
### 楼层 #2
**回复者:** 管理员
**时间:** 2026-08-10T09-00-00.000

二楼回复含 \`code\`
`;


describe('论坛 · 帖子正文解析', () => {
  it('parsePostContent 剥离元信息头并拆出楼层', () => {
    const parsed = parsePostContent(samplePost);
    expect(parsed.mainBody).toBe('主帖**正文**内容。');
    expect(parsed.mainBody).not.toContain('**作者:**');
    expect(parsed.floors).toHaveLength(2);
    expect(parsed.floors[0]).toMatchObject({ index: 1, author: '小冰' });
    expect(parsed.floors[0].body).toBe('一楼回复');
    expect(parsed.floors[1].author).toBe('管理员');
    expect(parsed.floors[1].body).toContain('`code`');
    expect(parsed.floors[0].timeMs).toBe(Date.parse('2026-08-10T08:30:00.000'));
  });

  it('无评论区时主帖兜底全文', () => {
    const parsed = parsePostContent('# 只有主帖\n\n正文');
    expect(parsed.floors).toHaveLength(0);
    expect(parsed.mainBody).toContain('正文');
  });
});

describe('论坛 · 渲染与展示工具', () => {
  it('renderForumMarkdown 渲染 Markdown 并过滤活动内容', () => {
    const html = renderForumMarkdown('**加粗**\n\n<script>alert(1)</script>');
    expect(html).toContain('<strong>加粗</strong>');
    expect(html).not.toContain('<script>');
  });

  it('renderForumMarkdown 拦截 javascript: 链接', () => {
    const html = renderForumMarkdown('[点我](javascript:alert(1))');
    expect(html).not.toContain('javascript:');
  });


});

describe('论坛 · Store 读写流', () => {
  beforeEach(() => {
    setActivePinia(createPinia());
    clearInvokeMocks();
  });



  it('loadDetail 缓存命中不重复请求，force 穿透', async () => {
    let calls = 0;
    mockInvoke('forum_get_post', () => {
      calls += 1;
      return { success: true, content: samplePost };
    });
    const store = useForumStore();
    const uid = '1724000000002-bbbb2222';

    await store.loadDetail(uid);
    await store.loadDetail(uid);
    expect(calls).toBe(1);

    await store.loadDetail(uid, true);
    expect(calls).toBe(2);
    expect(store.detailCache.get(uid)?.floors).toHaveLength(2);
  });

  it('reply 成功后失效缓存并重拉详情与列表', async () => {
    const calls: string[] = [];
    mockInvoke('forum_reply', () => {
      calls.push('reply');
      return { success: true };
    });
    mockInvoke('forum_get_post', () => {
      calls.push('detail');
      return { success: true, content: samplePost };
    });
    mockInvoke('forum_list_posts', () => {
      calls.push('list');
      return { success: true, posts: basePosts() };
    });

    const store = useForumStore();
    const uid = '1724000000002-bbbb2222';
    await store.loadDetail(uid);
    calls.length = 0;

    const ok = await store.reply(uid, '测试者', '一条回复');
    expect(ok).toBe(true);
    expect(calls).toEqual(['reply', 'detail', 'list']);
  });


});



describe('论坛删除（store.remove）', () => {
  beforeEach(() => {
    setActivePinia(createPinia());
    clearInvokeMocks();
  });

  it('删楼层携带 floor 参数；成功后详情缓存失效并重拉', async () => {
    const calls: Array<{ cmd: string; args: Record<string, unknown> }> = [];
    mockInvoke('forum_delete', (args) => {
      calls.push({ cmd: 'forum_delete', args: args as Record<string, unknown> });
      return { success: true };
    });
    mockInvoke('forum_get_post', () => {
      calls.push({ cmd: 'forum_get_post', args: {} });
      return { success: true, content: '# t\n\n**作者:** u\n\n---\n\n正文' };
    });
    mockInvoke('forum_list_posts', () => {
      calls.push({ cmd: 'forum_list_posts', args: {} });
      return { success: true, posts: [] };
    });

    const store = useForumStore();
    expect(await store.remove('post_uid_1', 3)).toBe(true);
    expect(calls[0]).toEqual({ cmd: 'forum_delete', args: { uid: 'post_uid_1', floor: 3 } });
    expect(calls.slice(1).map((c) => c.cmd).sort()).toEqual(['forum_get_post', 'forum_list_posts']);
  });

});

describe('parsePostContent 边界', () => {
  it('零评论帖子（文件以 评论区 分隔符直接结尾）不残留"评论区"标题', () => {
    // 上游 CreatePost 的初始文件：.trim() 后无尾换行
    const content = [
      '# 标题',
      '',
      '**作者:** 小娜',
      '**UID:** abc',
      '',
      '---',
      '',
      '正文内容',
      '',
      '---',
      '',
      '## 评论区',
      '---',
    ].join('\n');
    const parsed = parsePostContent(content);
    expect(parsed.mainBody).toBe('正文内容');
    expect(parsed.mainBody).not.toContain('评论区');
    expect(parsed.floors).toEqual([]);
  });
});
