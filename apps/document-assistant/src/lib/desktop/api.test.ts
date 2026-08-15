import { invoke } from '@tauri-apps/api/core';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { getAppInfo } from './api';

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn() }));

const invokeMock = vi.mocked(invoke);

describe('getAppInfo', () => {
  beforeEach(() => invokeMock.mockReset());

  it('maps the app_info command response', async () => {
    invokeMock.mockResolvedValue({ version: '0.1.0', appDataDir: 'C:\\Data' });

    await expect(getAppInfo()).resolves.toEqual({
      version: '0.1.0',
      appDataDir: 'C:\\Data',
    });
    expect(invokeMock).toHaveBeenCalledWith('get_app_info');
  });
});
