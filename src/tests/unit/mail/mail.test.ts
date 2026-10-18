import { beforeEach, describe, expect, it, vi } from 'vitest';
import { createPinia, setActivePinia } from 'pinia';
import { mailBodyOf, normalizeDetail, renderMailMarkdown } from '@/features/mail/mailTypes';
import { useMailStore } from '@/features/mail/mailStore';
import { useNotificationStore } from '@/core/stores/notification';
import { clearInvokeMocks, mockInvoke } from '../../mocks/tauri';

const testPlatform = vi.hoisted(() => ({ value: 'android' as 'android' | 'ios' }));
vi.mock('@/core/utils/platform', () => ({
  get nativePlatform() { return testPlatform.value; },
  get isAndroid() { return testPlatform.value === 'android'; },
  get isIOS() { return testPlatform.value === 'ios'; },
}));
beforeEach(() => { testPlatform.value = 'android'; });

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








  it('renderMailMarkdown 过滤活动内容', () => {
    const html = renderMailMarkdown('正文<script>alert(1)</script>');
    expect(html).toContain('正文');
    expect(html).not.toContain('<script>');
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

  it.each(['android', 'ios'] as const)('%s 附件导出入口不影响读取正文与附件元数据', async (platform) => {
    testPlatform.value = platform;
    mockBase();
    const calls: string[] = [];
    mockInvoke('mail_read', () => ({
      markdown: '', text: '邮件正文',
      attachments: [{ partId: 'part-1', filename: 'note.txt', contentType: 'text/plain', size: 4 }],
    }));
    mockInvoke('mail_attachment', () => {
      calls.push('download-and-save');
      return { uri: 'content://downloads/note.txt', displayName: 'note.txt', mimeType: 'text/plain', size: 4 };
    });
    mockInvoke('plugin:vcp-mobile|save_to_downloads', () => {
      calls.push('export');
      return { displayName: 'note.txt' };
    });
    const store = useMailStore();
    await store.startSession();
    try {
      await store.openDetail('msg_001');
      await store.downloadAttachment('msg_001', 'part-1', 'note.txt');
      expect(store.detail?.text).toBe('邮件正文');
      expect(store.detail?.attachments[0]?.filename).toBe('note.txt');
      expect(calls).toEqual(['download-and-save']);
    } finally {
      store.stopSession();
    }
  });

  it('附件导出进行中拒绝重复点击，收到原生保存结果后才提示成功', async () => {
    mockBase();
    const notice = vi.spyOn(useNotificationStore(), 'addNotification');
    let finish!: (value: unknown) => void;
    const save = vi.fn(() => new Promise((resolve) => { finish = resolve; }));
    mockInvoke('mail_attachment', save);
    const store = useMailStore();
    await store.startSession();
    try {
      const pending = store.downloadAttachment('msg_001', 'part-1', 'note.txt');
      await store.downloadAttachment('msg_001', 'part-1', 'note.txt');
      expect(save).toHaveBeenCalledTimes(1);
      expect(save).toHaveBeenCalledWith({
        mailId: 'msg_001', partId: 'part-1', filename: 'note.txt', user: 'bot@claw.163.com',
      });
      expect(notice.mock.calls.some(([notification]) => notification.type === 'success')).toBe(false);
      finish({ uri: 'content://downloads/note-1.txt', displayName: 'note-1.txt', mimeType: 'text/plain', size: 4 });
      await pending;
      expect(notice).toHaveBeenCalledWith(expect.objectContaining({
        type: 'success', message: '附件已保存到下载目录：note-1.txt',
      }));
    } finally {
      store.stopSession();
    }
  });

  it('保存失败保留具体错误，不重新下载，并允许用户随后再次操作', async () => {
    mockBase();
    const notice = vi.spyOn(useNotificationStore(), 'addNotification');
    const save = vi.fn()
      .mockRejectedValueOnce('保存附件失败: 未授予储存空间权限')
      .mockResolvedValueOnce({ displayName: 'note.txt' });
    mockInvoke('mail_attachment', save);
    const store = useMailStore();
    await store.startSession();
    try {
      await store.downloadAttachment('msg_001', 'part-1', 'note.txt');
      expect(save).toHaveBeenCalledTimes(1);
      expect(notice).toHaveBeenCalledWith(expect.objectContaining({
        type: 'error', message: '保存附件失败: 未授予储存空间权限',
      }));
      await store.downloadAttachment('msg_001', 'part-1', 'note.txt');
      expect(save).toHaveBeenCalledTimes(2);
    } finally {
      store.stopSession();
    }
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
