export function truncateFileName(name: string, maxLength: number = 22): string {
  if (!name || name.length <= maxLength) return name;

  const lastDotIndex = name.lastIndexOf('.');
  if (lastDotIndex <= 0) {
    return name.slice(0, maxLength - 1) + '…';
  }

  const baseName = name.slice(0, lastDotIndex);
  const ext = name.slice(lastDotIndex);
  const maxBaseLength = maxLength - ext.length - 1;

  if (maxBaseLength <= 3) {
    return name.slice(0, maxLength - 1) + '…';
  }

  return baseName.slice(0, maxBaseLength) + '…' + ext;
}
