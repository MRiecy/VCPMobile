import { defineAsyncComponent } from 'vue';
import { AttachmentType } from './types/AttachmentType';
import type { Component } from 'vue';

// Lazy load all attachment components
const componentMap = new Map<AttachmentType, Component>();

// Image component
componentMap.set(AttachmentType.IMAGE, defineAsyncComponent(() => 
  import('./types/ImageAttachment.vue')
));

// Video component
componentMap.set(AttachmentType.VIDEO, defineAsyncComponent(() => 
  import('./types/VideoAttachment.vue')
));

// Audio component
componentMap.set(AttachmentType.AUDIO, defineAsyncComponent(() => 
  import('./types/AudioAttachment.vue')
));

// Document component
componentMap.set(AttachmentType.DOCUMENT, defineAsyncComponent(() => 
  import('./types/DocumentAttachment.vue')
));

// Code component
componentMap.set(AttachmentType.CODE, defineAsyncComponent(() => 
  import('./types/CodeAttachment.vue')
));

// Text component
componentMap.set(AttachmentType.TEXT, defineAsyncComponent(() => 
  import('./types/TextAttachment.vue')
));

// Other component
componentMap.set(AttachmentType.OTHER, defineAsyncComponent(() => 
  import('./types/OtherAttachment.vue')
));

export class AttachmentFactory {
  /**
   * Creates a component instance based on attachment type
   */
  static createComponent(type: AttachmentType): Component | null {
    return componentMap.get(type) || null;
  }

  /**
   * Gets all registered component types
   */
  static getRegisteredTypes(): AttachmentType[] {
    return Array.from(componentMap.keys());
  }

  /**
   * Checks if a component is registered for a specific type
   */
  static hasComponent(type: AttachmentType): boolean {
    return componentMap.has(type);
  }
}