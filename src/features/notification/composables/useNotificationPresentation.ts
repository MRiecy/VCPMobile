import { computed } from 'vue';
import { Info, CheckCircle, AlertTriangle, X, Cpu, User } from 'lucide-vue-next';
import type { VcpNotification } from '../../../core/stores/notification';

export function useNotificationPresentation() {
  const iconMap = computed<Record<string, any>>(() => ({
    success: CheckCircle,
    warning: AlertTriangle,
    error: X,
    tool: Cpu,
    agent: User,
    info: Info
  }));

  const colorMap = {
    success: 'text-green-500',
    warning: 'text-amber-500',
    error: 'text-red-500',
    tool: 'text-purple-500',
    agent: 'text-blue-500',
    info: 'text-blue-400'
  } as const;

  const getIcon = (type: VcpNotification['type']) => (iconMap.value as any)[type] ?? Info;

  const getTypeColor = (type: VcpNotification['type']) => (colorMap as any)[type] ?? colorMap.info;
  const getActionButtonClass = (action: { label: string; color: string }) => {
    const toneClass = action.label === 'Approve' || action.color?.includes('green')
      ? 'bg-green-600'
      : action.label === 'Deny' || action.color?.includes('red')
        ? 'bg-red-600'
        : action.color;

    return [
      toneClass,
      'px-3 py-1.5 shadow-sm hover:shadow-md hover:-translate-y-0.5 active:translate-y-0 transition-all duration-200 font-medium text-[11px] rounded-lg text-white'
    ];
  };

  return {
    getIcon,
    getTypeColor,
    getActionButtonClass
  };
};
