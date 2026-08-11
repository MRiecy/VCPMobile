export class LatestIntentOwner {
  private currentId = "";
  private sequence = 0;

  begin(): string {
    const id = `local-share-${Date.now()}-${++this.sequence}`;
    this.currentId = id;
    return id;
  }

  isCurrent(id: string): boolean {
    return id !== "" && this.currentId === id;
  }

  clear(id: string): void {
    if (this.currentId === id) this.currentId = "";
  }
}
