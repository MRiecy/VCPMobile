import { VcpNotification } from '../stores/notification';

export function useNotificationProcessor() {
  /**
   * 对标桌面端 notificationRenderer.js 的解析逻辑
   * 负责将后端原始 JSON 转化为前端 UI 可用的结构
   */
  const processPayload = (payload: any): Partial<VcpNotification> => {
    let title = 'VCP 通知';
    let message = '';
    let type: VcpNotification['type'] = 'info';
    let isPreformatted = false;
    let duration = 7000; 
    let actions: VcpNotification['actions'] = [];

    // 1. 过滤桌面端也过滤的冗余信息
    if (payload.type === 'connection_ack' && payload.message?.includes('successful')) {
      return { silent: true };
    }

    // 2. 核心 VCP 日志解析 (对标 renderVCPLogNotification)
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
          // 提取助手名
          if (inner.MaidName) title += ` (${inner.MaidName})`;
          // 提取原始输出
          if (inner.original_plugin_output) {
            if (typeof inner.original_plugin_output === 'object') {
              message = JSON.stringify(inner.original_plugin_output, null, 2);
            } else {
              message = String(inner.original_plugin_output);
              isPreformatted = false;
            }
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
    // 3. 审批请求 (对标 L142)
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
    // 4. 视频生成状态
    else if (payload.type === 'video_generation_status') {
      type = 'info';
      title = '视频生成状态';
      message = payload.data?.original_plugin_output?.message || JSON.stringify(payload.data || {});
    }
    // 5. 默认回退
    else {
      title = payload.type || 'VCP Message';
      message = typeof payload === 'string' ? payload : (payload.message || JSON.stringify(payload));
    }

    // 统一截断 (L181)
    if (message.length > 500) {
      message = message.substring(0, 500) + '...';
    }

    return { title, message, type, isPreformatted, duration, actions, rawPayload: payload, silent: false };
  };

  return { processPayload };
}
