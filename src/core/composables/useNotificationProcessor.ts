import { VcpNotification, useNotificationStore, VcpStatus } from '../stores/notification';
import { findAgentMessagePayload, type AgentMessagePayload } from '../utils/agentMessagePayload';

/**
 * 过滤结果接口
 * action: 'show' 展示, 'hide' 拦截 (不推入 notificationStore)
 * duration: 可选覆盖默认显示时长
 */
export interface FilterResult {
  action: 'show' | 'hide';
  duration?: number;
  ruleName?: string;
}

/**
 * 过滤规则接口
 * match: 返回 true 表示命中规则
 */
export interface FilterRule {
  name: string;
  match: (title: string, message: string, payload: any) => boolean;
  action: 'show' | 'hide';
  duration?: number;
}

type AgentNotificationFields = {
  type: Extract<VcpNotification['type'], 'agent'>;
  title: string;
  message: string;
  isPreformatted: boolean;
  duration: number;
};

const hasAgentMessageContent = (
  agentPayload: AgentMessagePayload | null,
): agentPayload is AgentMessagePayload =>
  !!agentPayload && Boolean(agentPayload.message || agentPayload.originalContent);

const buildAgentNotificationFields = (
  agentPayload: AgentMessagePayload,
): AgentNotificationFields => {
  const message = String(agentPayload.message || agentPayload.originalContent);

  return {
    type: 'agent',
    title: agentPayload.title || (agentPayload.recipient ? `${agentPayload.recipient} 的消息` : 'Agent 消息'),
    message,
    isPreformatted: message.includes('\n'),
    duration: 10000
  };
};

export function useNotificationProcessor() {
  const store = useNotificationStore();

  /**
   * 全局消息过滤引擎 (对标桌面端 filterManager.js)
   * 允许根据标题、内容或原始负载拦截/修改消息展示行为
   */
  const checkMessageFilter = (title: string, message: string, payload: any): FilterResult => {
    // 初始内置降噪及增强规则
    const builtInRules: FilterRule[] = [
      {
        name: 'Heartbeat/Ping/Pong Noise Reduction',
        match: (t, m, p) => {
          const content = (t + m).toLowerCase();
          const pType = String(p?.type || '').toLowerCase();
          return (
            pType === 'heartbeat' || pType === 'ping' || pType === 'pong' ||
            content.includes('heartbeat') || content.includes('ping') || content.includes('pong')
          );
        },
        action: 'hide'
      },
      {
        name: 'Redundant Connection Success',
        match: (_t, m, p) =>
          p?.type === 'connection_ack' &&
          (m.toLowerCase().includes('successful') ||
            String(p?.message || '').toLowerCase().includes('successful') ||
            String(p?.data?.message || '').toLowerCase().includes('successful')),
        action: 'hide'
      },
      {
        name: 'Important Error Duration Extension',
        match: (t, m, p) =>
          t.toLowerCase().includes('error') ||
          m.toLowerCase().includes('failed') ||
          (p?.type === 'vcp-log-message' && p?.data?.status === 'error'),
        action: 'show',
        duration: 15000
      },
      {
        name: 'DistPluginManager Noise Reduction',
        match: (_t, m, p) =>
          p?.data?.source === 'DistPluginManager' &&
          (m.toLowerCase().includes('heartbeat') || m.toLowerCase().includes('checking server status')),
        action: 'hide'
      }
    ];

    for (const rule of builtInRules) {
      if (rule.match(title, message, payload)) {
        return {
          action: rule.action,
          duration: rule.duration,
          ruleName: rule.name
        };
      }
    }

    return { action: 'show' };
  };

  /**
   * 对标桌面端 notificationRenderer.js 的解析逻辑
   * 负责将后端原始 JSON 转化为前端 UI 可用的结构
   */
  const processPayload = (payload: any): Partial<VcpNotification> => {
    // 0. P2-7 Gap: 连接底层状态指示器 (VCPLog)
    // 同步状态不再渲染到全局状态栏（同步已改为完全手动触发，避免状态栏干扰）
    if (payload.type === 'vcp-log-status') {
      const statusData = payload.data || payload;
      const status = (statusData.status || 'connecting') as VcpStatus['status'];
      const source = statusData.source || 'VCPLog';
      const message = statusData.message || '状态未知';

      store.updateStatus({
        status,
        message,
        source
      });

      // 彻底静默连接状态通知
      return { silent: true };
    }

    // --- 核心引擎状态处理 (P0 级别) ---
    if (payload.type === 'vcp-core-status') {
      const { status, message } = payload;
      
      store.updateCoreStatus({ 
        status: status as any, 
        message: message || '核心状态变更',
        source: 'Core'
      });

      // 核心错误需要强制弹窗
      if (status === 'error') {
        return {
          id: 'vcp_core_fatal_error',
          title: '核心引擎异常',
          message: message || '后端服务发生未知崩溃',
          type: 'error',
          duration: 0
        };
      }
      return { silent: true };
    }

    let title = 'VCP 通知';
    let message = '';
    let type: VcpNotification['type'] = 'info';
    let isPreformatted = false;
    let duration = 7000;
    let actions: VcpNotification['actions'] = [];
    let notificationId: string | undefined = undefined;
    let historyOnly = false;
    const agentPayload = findAgentMessagePayload(payload);

    // --- 核心协议解析层 (对标桌面端 notificationRenderer.js) ---

    // 1. vcp_log: 核心工具调用日志 (服务端协议) 或 vcp-log-message (移动端内部兼容)
    if ((payload.type === 'vcp_log' || payload.type === 'vcp-log-message') && payload.data) {
      const vcpData = payload.data;
      
      if (vcpData.id) {
        notificationId = vcpData.id;
        if (vcpData.id === 'vcp_sync_connection_status' && vcpData.status === 'error') {
          historyOnly = true;
        }
      }

      if (hasAgentMessageContent(agentPayload)) {
        ({ type, title, message, isPreformatted, duration } = buildAgentNotificationFields(agentPayload));
      } else if (vcpData.tool_name && vcpData.status) {
        type = vcpData.status === 'error' 
          ? 'error' 
          : (vcpData.tool_name === 'DailyNote' ? 'success' : 'tool');
        
        const statusText = vcpData.status === 'success' ? '执行成功' : vcpData.status === 'error' ? '执行失败' : vcpData.status;
        title = `${vcpData.tool_name} ${statusText}`;

        let rawContent = String(vcpData.content || '');
        message = rawContent;
        
        // 智能降维渲染：如果文本内容以 Emoji ✅/❌ 开头，或者是不含有换行与大括号的单行日常提示
        // 则设为非 Preformatted，以便采用极致自然的原生排版呈现，剔除代码框的突兀感
        isPreformatted = !(
          rawContent.startsWith('✅') || 
          rawContent.startsWith('❌') || 
          (!rawContent.includes('\n') && !rawContent.includes('{'))
        );

        // 处理错误模式: "执行错误: {"plugin_error": "..."}"
        if (vcpData.status === 'error' && rawContent.includes('{')) {
          const jsonStart = rawContent.indexOf('{');
          const prefix = rawContent.substring(0, jsonStart).trim();
          const jsonPart = rawContent.substring(jsonStart);
          try {
            const parsed = JSON.parse(jsonPart);
            const errorMsg = parsed.plugin_error || parsed.error || parsed.message;
            if (errorMsg) {
              message = prefix ? `${prefix}${prefix.endsWith(':') ? ' ' : ': '}${errorMsg}` : errorMsg;
              isPreformatted = false;
            }
          } catch (e) { }
        }

        // 尝试解析内部元数据 (MaidName, timestamp)
        try {
          const inner = JSON.parse(rawContent);
          let titleSuffix = '';
          if (inner.MaidName) {
            titleSuffix += ` by ${inner.MaidName}`;
          }
          if (inner.timestamp && typeof inner.timestamp === 'string' && inner.timestamp.length >= 16) {
            const timeStr = inner.timestamp.substring(11, 16);
            titleSuffix += `${inner.MaidName ? ' ' : ''}@ ${timeStr}`;
          }
          if (titleSuffix) {
            title += ` (${titleSuffix.trim()})`;
          }

          if (typeof inner.original_plugin_output !== 'undefined') {
            if (typeof inner.original_plugin_output === 'object' && inner.original_plugin_output !== null) {
              message = JSON.stringify(inner.original_plugin_output, null, 2);
            } else {
              message = String(inner.original_plugin_output);
              isPreformatted = false;
            }
          } else if (vcpData.tool_name === 'DailyNote' && vcpData.status === 'success') {
            message = "✅ 日记内容已成功记录到本地知识库。";
            isPreformatted = false;
          }
        } catch (e) { }
      } else if (vcpData.source === 'DistPluginManager' || vcpData.source === 'Distributed') {
        title = '分布式服务器';
        message = vcpData.content || JSON.stringify(vcpData);
        isPreformatted = false;
      } else {
        title = 'VCP 日志条目';
        message = JSON.stringify(vcpData, null, 2);
        isPreformatted = true;
      }
    }
    // 2. video_generation_status: 视频生成状态
    else if (payload.type === 'video_generation_status' && payload.data) {
      type = 'info';
      title = '视频生成状态';
      const vData = payload.data;

      if (vData.original_plugin_output && typeof vData.original_plugin_output.message === 'string') {
        message = vData.original_plugin_output.message;
        isPreformatted = false;
      } else if (vData.original_plugin_output) {
        message = JSON.stringify(vData.original_plugin_output, null, 2);
        isPreformatted = true;
      } else {
        message = JSON.stringify(vData, null, 2);
        isPreformatted = true;
      }

      if (vData.timestamp && typeof vData.timestamp === 'string' && vData.timestamp.length >= 16) {
        title += ` (@ ${vData.timestamp.substring(11, 16)})`;
      }
    }
    // 3. daily_note_created: 日记创建通知
    else if (payload.type === 'daily_note_created' && payload.data) {
      const noteData = payload.data;
      title = `日记: ${noteData.maidName || 'N/A'} (${noteData.dateString || 'N/A'})`;
      type = noteData.status === 'success' ? 'success' : 'info';
      message = noteData.message || (noteData.status === 'success' ? '日记已成功创建。' : `日记处理状态: ${noteData.status || '未知'}`);
      isPreformatted = false;
    }
    // 4. connection_ack: 连接确认
    else if (payload.type === 'connection_ack' && payload.message) {
      title = 'VCP 连接';
      message = String(payload.message);
      isPreformatted = false;
    }
    // 5. agent_message: 移动端本机 AgentMessage 推送
    else if (hasAgentMessageContent(agentPayload)) {
      ({ type, title, message, isPreformatted, duration } = buildAgentNotificationFields(agentPayload));
    }
    // 6. tool_approval_request: 审核请求
    else if (payload.type === 'tool_approval_request' && payload.data) {
      const approvalData = payload.data;
      type = 'warning';
      title = `🛠️ 审核请求: ${approvalData.toolName || 'Unknown'}`;
      message = `助手: ${approvalData.maid || 'N/A'}\n命令: ${approvalData.args?.command || JSON.stringify(approvalData.args || {})}\n时间: ${approvalData.timestamp || 'Just now'}`;
      isPreformatted = true;
      duration = 0;
      actions = [
        { label: '允许', value: true, color: 'bg-green-500 shadow-lg shadow-green-500/20' },
        { label: '拒绝', value: false, color: 'bg-red-500 shadow-lg shadow-red-500/20' }
      ];
    }
    // 7. 默认回退 (Generic fallback)
    else {
      if (typeof payload === 'object' && payload !== null) {
        title = payload.type ? `类型: ${payload.type}` : 'VCP 消息';
        message = payload.message || (payload.data?.message) || JSON.stringify(payload, null, 2);
        
        // 如果有附加数据，追加展示
        if (payload.data && !payload.message) {
          message = JSON.stringify(payload.data, null, 2);
          isPreformatted = true;
        } else {
          isPreformatted = message.includes('{') || message.includes('\n');
        }
      } else {
        title = 'VCP 消息';
        message = String(payload);
        isPreformatted = false;
      }
    }

    // 统一截断 (L181)
    if (message.length > 300) {
      message = message.substring(0, 300) + '...';
    }

    // 5. 执行全局过滤引擎 (P0-1 功能)
    const filterResult = checkMessageFilter(title, message, payload);

    if (filterResult.action === 'hide') {
      return { silent: true };
    }

    const result: Partial<VcpNotification> = {
      title,
      message,
      type,
      isPreformatted,
      duration: filterResult.duration ?? duration,
      actions,
      rawPayload: payload,
      silent: false,
      historyOnly
    };

    if (notificationId) {
      result.id = notificationId;
    }

    return result;
  };

  return { processPayload };
}
