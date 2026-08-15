import { invoke } from '@tauri-apps/api/core';
import type { AppInfo } from './types';

export function invokeCommand<T>(
  command: string,
  args?: Record<string, unknown>,
): Promise<T> {
  return invoke<T>(command, args);
}

export function getAppInfo(): Promise<AppInfo> {
  return invokeCommand<AppInfo>('get_app_info');
}
