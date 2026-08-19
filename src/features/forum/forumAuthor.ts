/**
 * forumAuthor.ts — 论坛署名 → 本地实体头像解析。
 *
 * 论坛没有作者账号体系，署名（Maid）只是字符串；但发帖的 Agent 是本地
 * 真实实体（assistant store 的 agents），用户本人署名 = 设置中的用户名。
 * 按名字精确匹配到本地实体后用其真实头像（VcpAvatar 管线，含缓存与
 * 主色调），匹配不到才回退首字母占位。
 */
import { useAssistantStore } from '../../core/stores/assistant';
import { useSettingsStore } from '../../core/stores/settings';
import type { AvatarTarget } from '../../components/ui/VcpAvatar.vue';

/** 署名 → 头像目标；无法解析为本地实体时返回 null（调用方用占位头像）。 */
export function resolveForumAuthor(author: string): AvatarTarget | null {
  const name = author.trim();
  if (!name) return null;

  const settingsStore = useSettingsStore();
  if (settingsStore.settings?.userName?.trim() === name) {
    return { type: 'user', id: 'user_avatar', name };
  }

  const assistantStore = useAssistantStore();
  const agent = assistantStore.agents.find((entry) => entry.name.trim() === name);
  if (agent) {
    return {
      type: 'agent',
      id: agent.id,
      name,
      avatarCalculatedColor: agent.avatarCalculatedColor ?? null,
    };
  }
  return null;
}
