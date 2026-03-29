import { VcpNotification, useNotificationStore, VcpStatus } from '../stores/notification';

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
          (p?.type === 'vcp_log' && p?.data?.status === 'error'),
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
    // 0. P2-7 Gap: 连接底层状态指示器 (vcp_log_status / connection_status)
    if (payload.type === 'vcp_log_status' || payload.type === 'connection_status') {
      const statusData = payload.data || payload;
      store.updateStatus({
        status: (statusData.status || 'connecting') as VcpStatus['status'],
        message: statusData.message || '状态未知',
        source: statusData.source || 'VCPLog'
      });
      return { silent: true };
    }

    let title = 'VCP 通知';
    let message = '';
    let type: VcpNotification['type'] = 'info';
    let isPreformatted = false;
    let duration = 7000; 
    let actions: VcpNotification['actions'] = [];

    // 1. 核心 VCP 日志解析 (对标 renderVCPLogNotification)
    if (payload.type === 'vcp_log' && payload.data) {
      const vcpData = payload.data;
      if (vcpData.tool_name && vcpData.status) {
        type = vcpData.status === 'error' ? 'error' : 'tool';
        title = `${vcpData.tool_name} ${vcpData.status}`;
        
        let rawContent = String(vcpData.content || '');
        message = rawContent;
        isPreformatted = true;

        // 尝试深层解析
        try {
          const inner = JSON.parse(rawContent);

          // P1-5 Gap: 提取内部时间戳并聚合标题 (对标桌面端 L61-68)
          const ts = inner.timestamp;
          if (ts && typeof ts === 'string' && ts.length >= 16) {
            const timeStr = ts.substring(11, 16);
            if (inner.MaidName) {
              title += ` (by ${inner.MaidName} @ ${timeStr})`;
            } else {
              title += ` (@ ${timeStr})`;
            }
          } else if (inner.MaidName) {
            title += ` (${inner.MaidName})`;
          }
          
          let hasValidOutput = false;
          // 提取原始输出
          if (inner.original_plugin_output) {
            if (typeof inner.original_plugin_output === 'object') {
              message = JSON.stringify(inner.original_plugin_output, null, 2);
              hasValidOutput = true;
            } else if (String(inner.original_plugin_output).trim()) {
              message = String(inner.original_plugin_output);
              isPreformatted = false;
              hasValidOutput = true;
            }
          }

          // DailyNote 成功状态 Fallback (P1-4 Gap)
          if (!hasValidOutput && vcpData.tool_name === 'DailyNote' && vcpData.status === 'success') {
            message = "✅ 日记内容已成功记录到本地知识库。";
            isPreformatted = false;
          }
        } catch (e) {
            // 解析失败则保持 rawContent
        }

        // 错误模式处理 (针对嵌套的 JSON 错误)
        if (vcpData.status === 'error' && rawContent.includes('{')) {
          try {
            const jsonPart = rawContent.substring(rawContent.indexOf('{'));
            const parsed = JSON.parse(jsonPart);
            const errorMsg = parsed.plugin_error || parsed.error || parsed.message;
            if (errorMsg) {
              message = errorMsg;
              isPreformatted = false;
            }
          } catch (e) {}
        }
      } else if (vcpData.source === 'DistPluginManager') {
        title = '分布式服务器';
        message = vcpData.content || JSON.stringify(vcpData);
      }
    } 
    // 2. 审批请求 (对标 L142)
    else if (payload.type === 'tool_approval_request') {
      const approvalData = payload.data;
      type = 'warning';
      title = `🛠️ 审核请求: ${approvalData.toolName || 'Unknown'}`;
      message = `助手: ${approvalData.maid || 'N/A'}\n命令: ${approvalData.args?.command || JSON.stringify(approvalData.args || {})}\n时间: ${approvalData.timestamp || 'Just now'}`;
      isPreformatted = true;
      duration = 0; // 永不自动消失
      actions = [
        { label: '允许', value: true, color: 'bg-green-500 shadow-lg shadow-green-500/20' },
        { label: '拒绝', value: false, color: 'bg-red-500 shadow-lg shadow-red-500/20' }
      ];
    }
    // 3. 视频生成状态 (对标桌面端 L93-97)
    else if (payload.type === 'video_generation_status') {
      type = 'info';
      title = '视频生成状态';
      
      const vTs = payload.data?.timestamp;
      if (vTs && typeof vTs === 'string' && vTs.length >= 16) {
        title += ` (@ ${vTs.substring(11, 16)})`;
      }
      
      message = payload.data?.original_plugin_output?.message || JSON.stringify(payload.data || {});
    }
    // 4. 日记创建状态 (对标桌面端 L118)
    else if (payload.type === 'daily_note_created') {
      const noteData = payload.data || {};
      title = `日记: ${noteData.maidName || 'N/A'} (${noteData.dateString || 'N/A'})`;
      
      if (noteData.status === 'success') {
        type = 'success';
        message = noteData.message || '日记已成功创建。';
      } else {
        type = 'info';
        message = noteData.message || '日记处理状态: ' + (noteData.status || '未知');
      }
    }
    // 5. 默认回退
    else {
      title = payload.type || 'VCP Message';
      message = typeof payload === 'string' ? payload : (payload.message || JSON.stringify(payload));
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

    return { 
      title, 
      message, 
      type, 
      isPreformatted, 
      duration: filterResult.duration ?? duration, 
      actions, 
      rawPayload: payload, 
      silent: false 
    };
  };

  return { processPayload };
}
