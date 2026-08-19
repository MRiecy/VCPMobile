import { beforeEach, describe, expect, it } from 'vitest';
import { createPinia, setActivePinia } from 'pinia';
import {
  addressingOf,
  extractDetailMarkdown,
  folderDisplayName,
  folderKindOf,
  mailBodyOf,
  mailPartyText,
  mailTimeLabel,
  normalizeDetail,
  normalizeFolders,
  normalizeMailboxes,
  normalizeMailList,
  normalizeMailSummary,
  normalizeWsStates,
  organizeFolders,
  parseMailDate,
  renderMailMarkdown,
} from '@/features/mail/mailTypes';
import { useMailStore } from '@/features/mail/mailStore';
import { clearInvokeMocks, mockInvoke } from '../../mocks/tauri';

const baseState = () => ({
  status: 'success',
  sdkLoaded: true,
  updatedAt: '2026-08-19T10:00:00.000',
  lastError: null,
  mailboxes: [
    { user: 'bot@claw.163.com', mailbox: 'public', label: 'bot@claw.163.com', agentName: null, enabled: true, cachedCount: 12 },
    { user: 'sub1@claw.163.com', mailbox: 'mail1', label: 'mail1', agentName: '小娜', enabled: true, cachedCount: 3 },
  ],
  users: {},
  wsStates: [
    { user: 'bot@claw.163.com', connected: true },
    { user: 'sub1@claw.163.com', connected: false, lastError: 'ws down' },
  ],
});

const baseMails = () => [
  {
    user: 'bot@claw.163.com',
    mailId: 'msg_001',
    subject: 'CI 构建失败',
    from: { name: 'GitHub', address: 'noreply@github.com' },
    to: ['bot@claw.163.com'],
    date: '2026-08-19T09:30:00+08:00',
    read: false,
    hasAttachments: true,
    preview: 'Run failed: tests',
  },
  {
    user: 'bot@claw.163.com',
    mailId: 'msg_002',
    subject: null,
    from: 'plain@example.com',
    date: 'not-a-date',
    hasAttachments: false,
    preview: '你好',
  },
];

describe('邮箱 · 归一化层', () => {
  it('normalizeMailboxes 解析 mailboxes 数组', () => {
    const boxes = normalizeMailboxes(baseState().mailboxes);
    expect(boxes).toHaveLength(2);
    expect(boxes[0]).toMatchObject({ mailbox: 'public', user: 'bot@claw.163.com', agentName: null });
    expect(boxes[1].agentName).toBe('小娜');
  });

  it('normalizeWsStates 解析在线状态', () => {
    const states = normalizeWsStates(baseState().wsStates);
    expect(states[0].connected).toBe(true);
    expect(states[1].connected).toBe(false);
    expect(states[1].lastError).toBe('ws down');
  });

  it('addressingOf：子邮箱传 mailbox 槽位，公共邮箱传 user 地址', () => {
    const boxes = normalizeMailboxes(baseState().mailboxes);
    expect(addressingOf(boxes[0])).toEqual({ user: 'bot@claw.163.com' });
    expect(addressingOf(boxes[1])).toEqual({ mailbox: 'mail1' });
  });

  it('mailPartyText 宽容处理 string/array/object', () => {
    expect(mailPartyText('a@b.com')).toBe('a@b.com');
    expect(mailPartyText(['a@b.com', 'c@d.com'])).toBe('a@b.com, c@d.com');
    expect(mailPartyText({ name: 'GitHub', address: 'noreply@github.com' })).toBe(
      'GitHub <noreply@github.com>',
    );
    expect(mailPartyText({ address: 'x@y.z' })).toBe('x@y.z');
    expect(mailPartyText(null)).toBe('');
  });

  it('parseMailDate 宽容解析', () => {
    expect(parseMailDate('2026-08-19T09:30:00+08:00')).toBe(
      Date.parse('2026-08-19T09:30:00+08:00'),
    );
    expect(parseMailDate('not-a-date')).toBe(0);
    expect(parseMailDate(undefined)).toBe(0);
    expect(parseMailDate(1724000000000)).toBe(1724000000000);
  });

  it('normalizeMailSummary 三态已读 + 缺省兜底', () => {
    const mails = normalizeMailList(baseMails());
    expect(mails[0].readState).toBe('unread');
    expect(mails[0].fromText).toBe('GitHub <noreply@github.com>');
    expect(mails[0].hasAttachments).toBe(true);
    // subject null → '(无主题)'；read/unread 双缺 → unknown；坏 date → 0
    expect(mails[1].subject).toBe('(无主题)');
    expect(mails[1].readState).toBe('unknown');
    expect(mails[1].dateMs).toBe(0);
  });

  it('normalizeMailSummary 从 unread 字段反推', () => {
    expect(normalizeMailSummary({ mailId: 'x', unread: true })?.readState).toBe('unread');
    expect(normalizeMailSummary({ mailId: 'x', unread: false })?.readState).toBe('read');
  });

  it('extractDetailMarkdown 提取 markdown 字段', () => {
    expect(extractDetailMarkdown({ status: 'success', markdown: '# 正文' })).toBe('# 正文');
    expect(extractDetailMarkdown({})).toBe('');
  });

  it('renderMailMarkdown 过滤活动内容', () => {
    const html = renderMailMarkdown('正文<script>alert(1)</script>');
    expect(html).toContain('正文');
    expect(html).not.toContain('<script>');
  });

  it('mailTimeLabel 今天显示时分', () => {
    const now = Date.now();
    expect(mailTimeLabel(now)).toMatch(/^\d{2}:\d{2}$/);
    expect(mailTimeLabel(0)).toBe('—');
  });
});

describe('邮箱 · Store 读写流', () => {
  beforeEach(() => {
    setActivePinia(createPinia());
    clearInvokeMocks();
  });

  function mockBase() {
    mockInvoke('mail_state', () => baseState());
    mockInvoke('mail_list', (args) => ({
      status: 'success',
      meta: {},
      emails: Number(args?.start) > 0 ? [] : baseMails(),
      markdown: '',
    }));
  }

  it('startSession 加载 state 并默认选中首个启用邮箱', async () => {
    mockBase();
    const store = useMailStore();
    await store.startSession();

    expect(store.stateLoaded).toBe(true);
    expect(store.selectedKey).toBe('bot@claw.163.com');
    expect(store.wsConnected).toBe(true);
    expect(store.mails).toHaveLength(2);
    store.stopSession();
  });

  it('切换子邮箱时使用 mailbox 槽位寻址', async () => {
    let listArgs: Record<string, unknown> | undefined;
    mockInvoke('mail_state', () => baseState());
    mockInvoke('mail_list', (args) => {
      listArgs = args;
      return { status: 'success', emails: [], markdown: '' };
    });

    const store = useMailStore();
    await store.startSession();
    await store.selectMailbox('mail1');

    expect(listArgs?.mailbox).toBe('mail1');
    expect(listArgs?.user).toBeUndefined();
    store.stopSession();
  });

  it('loadMore 以 start=当前长度增量加载', async () => {
    const starts: number[] = [];
    mockInvoke('mail_state', () => baseState());
    mockInvoke('mail_list', (args) => {
      starts.push(Number(args?.start));
      return { status: 'success', emails: Number(args?.start) === 0 ? baseMails() : [baseMails()[0]], markdown: '' };
    });

    const store = useMailStore();
    await store.startSession();
    await store.loadList(false);

    expect(starts).toEqual([0, 2]);
    expect(store.mails).toHaveLength(3);
    store.stopSession();
  });

  it('trash 成功后关闭详情并重拉列表', async () => {
    mockBase();
    mockInvoke('mail_read', () => ({ status: 'success', markdown: '正文', meta: {} }));
    mockInvoke('mail_trash', () => ({ status: 'success', meta: {}, markdown: '' }));

    const store = useMailStore();
    await store.startSession();
    await store.openDetail('msg_001');
    expect(store.detailMailId).toBe('msg_001');

    const ok = await store.trash('msg_001');
    expect(ok).toBe(true);
    expect(store.detailMailId).toBeNull();
    store.stopSession();
  });

  it('503 映射为插件不可用专态', async () => {
    mockInvoke('mail_state', () =>
      Promise.reject(new Error('PLUGIN_UNAVAILABLE:VCPClawMail 插件未加载')),
    );
    const store = useMailStore();
    await store.startSession();

    expect(store.pluginUnavailable).toBe(true);
    expect(store.error).toBe('VCPClawMail 插件未加载');
    store.stopSession();
  });
});

describe('邮箱 · V1.1（补丁端点）', () => {
  beforeEach(() => {
    setActivePinia(createPinia());
    clearInvokeMocks();
  });

  function mockBase() {
    mockInvoke('mail_state', () => baseState());
    mockInvoke('mail_list', () => ({
      status: 'success',
      meta: {},
      emails: baseMails(),
      markdown: '',
    }));
  }

  it('normalizeFolders 与 normalizeAttachments', async () => {
    const { normalizeFolders, normalizeAttachments } = await import('@/features/mail/mailTypes');
    expect(normalizeFolders([
      { id: '1', name: 'INBOX', unreadCount: 3 },
      { fid: '5', name: 'Archive' },
      { name: '无id丢弃' },
    ])).toEqual([
      { id: '1', name: 'INBOX', unreadCount: 3 },
      { id: '5', name: 'Archive', unreadCount: null },
    ]);
    expect(normalizeAttachments([
      { partId: '2', filename: 'a.pdf', contentType: 'application/pdf', size: 1024 },
      { attachmentId: 'att-1', filename: 'b.png', cid: 'cid1' },
      { filename: '无id丢弃' },
    ]).map((att) => att.partId)).toEqual(['2', 'att-1']);
  });

  it('文件夹选择与 fid 透传', async () => {
    let listArgs: Record<string, unknown> | undefined;
    mockInvoke('mail_state', () => baseState());
    mockInvoke('mail_folders', () => ({
      status: 'success',
      meta: {},
      folders: [{ id: '4', name: '已发送' }],
    }));
    mockInvoke('mail_list', (args) => {
      listArgs = args;
      return { status: 'success', emails: [], markdown: '' };
    });

    const store = useMailStore();
    await store.startSession();
    expect(store.folders.map((folder) => folder.name)).toEqual(['已发送']);

    await store.selectFolder('4');
    expect(listArgs?.fid).toBe('4');
    store.stopSession();
  });

  it('文件夹端点 404 时降级隐藏扩展 UI', async () => {
    mockInvoke('mail_state', () => baseState());
    mockInvoke('mail_list', () => ({ status: 'success', emails: [], markdown: '' }));
    mockInvoke('mail_folders', () => Promise.reject(new Error('邮箱操作失败: Cannot GET /admin_api/claw-mail/folders')));

    const store = useMailStore();
    await store.startSession();
    await new Promise((resolve) => setTimeout(resolve, 0));

    expect(store.extendedApiSupported).toBe(false);
    store.stopSession();
  });

  it('搜索模式替换列表展示，清除后恢复', async () => {
    mockBase();
    mockInvoke('mail_search', (args) => ({
      status: 'success',
      meta: {},
      emails: args?.keyword === 'CI' ? [baseMails()[0]] : [],
      markdown: '',
    }));

    const store = useMailStore();
    await store.startSession();

    await store.search('CI');
    expect(store.displayedMails.map((mail) => mail.mailId)).toEqual(['msg_001']);

    store.clearSearch();
    expect(store.displayedMails).toHaveLength(2);
    store.stopSession();
  });

  it('setRead(false) 标为未读并同步列表状态', async () => {
    mockBase();
    let markArgs: Record<string, unknown> | undefined;
    mockInvoke('mail_mark', (args) => {
      markArgs = args;
      return { status: 'success', meta: {}, markdown: '' };
    });
    mockInvoke('mail_read', () => ({ status: 'success', markdown: '正文', meta: {}, attachments: [] }));

    const store = useMailStore();
    await store.startSession();
    await store.openDetail('msg_002');
    await store.setRead(true);

    expect(markArgs?.read).toBe(true);
    expect(store.mails.find((mail) => mail.mailId === 'msg_002')?.readState).toBe('read');
    store.stopSession();
  });

  it('sendMail 成功重拉列表；replyMail 标读原邮件', async () => {
    const calls: string[] = [];
    mockInvoke('mail_state', () => baseState());
    mockInvoke('mail_list', () => {
      calls.push('list');
      return { status: 'success', emails: baseMails(), markdown: '' };
    });
    mockInvoke('mail_send', () => {
      calls.push('send');
      return { status: 'success', meta: {}, markdown: '' };
    });
    mockInvoke('mail_reply', () => {
      calls.push('reply');
      return { status: 'success', meta: {}, markdown: '' };
    });

    const store = useMailStore();
    await store.startSession();
    calls.length = 0;

    expect(await store.sendMail({ to: 'a@b.com', subject: 's', body: 'b' })).toBe(true);
    expect(calls).toEqual(['send', 'list']);

    expect(await store.replyMail('msg_001', '回复正文')).toBe(true);
    expect(store.mails.find((mail) => mail.mailId === 'msg_001')?.readState).toBe('read');
    store.stopSession();
  });
});


describe('邮件详情正文提取（mailBodyOf）', () => {
  it('优先使用结构化 html 字段', () => {
    const detail = normalizeDetail({
      markdown: '# ClawEmail 邮件详情\n## 基本信息\n- 主题：x',
      html: '<p>你好</p>',
      text: '你好',
      attachments: [],
    });
    expect(mailBodyOf(detail)).toEqual({ kind: 'html', html: '<p>你好</p>' });
  });

  it('无 html 时使用纯文本正文', () => {
    const detail = normalizeDetail({ markdown: '', html: null, text: '第一行\n第二行', attachments: [] });
    expect(mailBodyOf(detail)).toEqual({ kind: 'text', text: '第一行\n第二行' });
  });

  it('旧服务器兜底：从 AI markdown 中只提取正文段落', () => {
    const aiMarkdown = [
      '# ClawEmail 邮件详情',
      '',
      '## 基本信息',
      '',
      '- 邮箱：bot@claw.163.com',
      '- mailId：`msg_1`',
      '',
      '## HTML 正文转 Markdown',
      '',
      '真正的正文内容',
      '',
      '## 附件',
      '',
      '无附件。',
      '',
      '## 可执行后续动作',
      '',
      '- 回复：调用 reply_mail',
    ].join('\n');
    const detail = normalizeDetail({ markdown: aiMarkdown, attachments: [] });
    const body = mailBodyOf(detail);
    expect(body?.kind).toBe('markdown');
    if (body?.kind === 'markdown') {
      expect(body.markdown).toBe('真正的正文内容');
      expect(body.markdown).not.toContain('基本信息');
      expect(body.markdown).not.toContain('可执行后续动作');
    }
  });

  it('空详情返回 null', () => {
    expect(mailBodyOf(normalizeDetail({}))).toBeNull();
  });
});

describe('文件夹语义化（organizeFolders / folderDisplayName / folderKindOf）', () => {
  it('收件箱去重 + 草稿箱隐藏 + 系统文件夹中文化排序', () => {
    const folders = normalizeFolders([
      { id: '1', name: 'INBOX' },
      { id: '2', name: 'Trash' },
      { id: '3', name: 'Sent Messages' },
      { id: '4', name: '项目归档2026' },
      { id: '5', name: 'Drafts' },
      { id: '6', name: '病毒文件夹' },
    ]);
    const organized = organizeFolders(folders);
    // INBOX 去重（固定入口）；Drafts 隐藏（SDK 无草稿 API）；
    // 排序：已发送 → 病毒文件夹 → 已删除 → 自定义
    expect(organized.map((folder) => folder.id)).toEqual(['3', '6', '2', '4']);
    expect(folderDisplayName('Sent Messages')).toBe('已发送');
    expect(folderDisplayName('Trash')).toBe('已删除');
    expect(folderDisplayName('垃圾邮箱')).toBe('垃圾邮箱');
    // 自定义文件夹保留原名
    expect(folderDisplayName('项目归档2026')).toBe('项目归档2026');
  });

  it('中文服务器文件夹名同样正确归类（真实去重场景）', () => {
    const folders = normalizeFolders([
      { id: '1', name: '收件箱' },
      { id: '2', name: '已发送' },
      { id: '3', name: '已删除' },
    ]);
    expect(organizeFolders(folders).map((folder) => folder.id)).toEqual(['2', '3']);
    expect(folderKindOf('已删除')).toBe('trash');
    expect(folderKindOf('收件箱')).toBe('inbox');
  });
});


describe('从已删除恢复（restoreToInbox）', () => {
  beforeEach(() => {
    setActivePinia(createPinia());
    clearInvokeMocks();
  });

  it('移回收件箱 = mail_move 到收件箱文件夹 id', async () => {
    const moved: Record<string, unknown> = {};
    mockInvoke('mail_state', () => baseState());
    mockInvoke('mail_list', () => ({ status: 'success', emails: baseMails(), markdown: '' }));
    mockInvoke('mail_folders', () => ({
      status: 'success',
      folders: [
        { id: 'f-inbox', name: '收件箱' },
        { id: 'f-trash', name: '已删除' },
      ],
    }));
    mockInvoke('mail_move', (args) => {
      Object.assign(moved, args);
      return { status: 'success' };
    });

    const store = useMailStore();
    await store.startSession();
    // 等 loadFolders 完成
    await new Promise((resolve) => setTimeout(resolve, 0));

    expect(store.currentFolderKind).toBe('inbox');
    await store.selectFolder('f-trash');
    expect(store.currentFolderKind).toBe('trash');

    expect(await store.restoreToInbox('msg_001')).toBe(true);
    expect(moved).toMatchObject({ mailId: 'msg_001', target: 'f-inbox', user: 'bot@claw.163.com' });
    store.stopSession();
  });

  it('服务器未返回收件箱文件夹时不可恢复', async () => {
    mockInvoke('mail_state', () => baseState());
    mockInvoke('mail_list', () => ({ status: 'success', emails: [], markdown: '' }));
    mockInvoke('mail_folders', () => ({
      status: 'success',
      folders: [{ id: 'f-trash', name: '已删除' }],
    }));

    const store = useMailStore();
    await store.startSession();
    await new Promise((resolve) => setTimeout(resolve, 0));
    expect(store.inboxFolderId).toBeNull();
    expect(await store.restoreToInbox('msg_001')).toBe(false);
    store.stopSession();
  });
});
