import { defineAsyncComponent } from 'vue';
import { AttachmentType } from './types/AttachmentType';
import type { Component } from 'vue';

/**
 * Registry for managing attachment component mappings
 * Allows runtime registration and retrieval of attachment components
 */
export class AttachmentRegistry {
  private static componentMap = new Map<AttachmentType, Component>();

  /**
   * Register a component for a specific attachment type
   * @param type - The attachment type
   * @param component - The Vue component to register
   */
  static register(type: AttachmentType, component: Component): void {
    this.componentMap.set(type, component);
    console.log(`[AttachmentRegistry] Registered component for type: ${type}`);
  }

  /**
   * Register multiple component mappings at once
   * @param mappings - Object mapping attachment types to components
   */
  static registerMultiple(mappings: Record<AttachmentType, Component>): void {
    Object.entries(mappings).forEach(([type, component]) => {
      this.register(type as AttachmentType, component);
    });
  }

  /**
   * Get a component for a specific attachment type
   * @param type - The attachment type
   * @returns The component or null if not found
   */
  static getComponent(type: AttachmentType): Component | null {
    return this.componentMap.get(type) || null;
  }

  /**
   * Check if a component is registered for a specific type
   * @param type - The attachment type
   * @returns True if component exists, false otherwise
   */
  static hasComponent(type: AttachmentType): boolean {
    return this.componentMap.has(type);
  }

  /**
   * Get all registered attachment types
   * @returns Array of registered attachment types
   */
  static getRegisteredTypes(): AttachmentType[] {
    return Array.from(this.componentMap.keys());
  }

  /**
   * Remove a component registration
   * @param type - The attachment type to remove
   */
  static unregister(type: AttachmentType): void {
    this.componentMap.delete(type);
    console.log(`[AttachmentRegistry] Unregistered component for type: ${type}`);
  }

  /**
   * Clear all component registrations
   */
  static clear(): void {
    this.componentMap.clear();
    console.log('[AttachmentRegistry] Cleared all component registrations');
  }

  /**
   * Get the total number of registered components
   */
  static get count(): number {
    return this.componentMap.size;
  }
}

// Register default components on import
AttachmentRegistry.registerMultiple({
  [AttachmentType.IMAGE]: defineAsyncComponent(() => 
    import('./types/ImageAttachment.vue')
  ),
  [AttachmentType.VIDEO]: defineAsyncComponent(() => 
    import('./types/VideoAttachment.vue')
  ),
  [AttachmentType.AUDIO]: defineAsyncComponent(() => 
    import('./types/AudioAttachment.vue')
  ),
  [AttachmentType.DOCUMENT]: defineAsyncComponent(() => 
    import('./types/DocumentAttachment.vue')
  ),
  [AttachmentType.CODE]: defineAsyncComponent(() => 
    import('./types/CodeAttachment.vue')
  ),
  [AttachmentType.TEXT]: defineAsyncComponent(() => 
    import('./types/TextAttachment.vue')
  ),
  [AttachmentType.OTHER]: defineAsyncComponent(() => 
    import('./types/OtherAttachment.vue')
  ),
});
