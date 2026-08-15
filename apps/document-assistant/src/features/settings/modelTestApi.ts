import { invokeCommand } from '../../lib/desktop/api';

export interface ModelTestResult {
  ok: boolean;
  message: string;
}

export function testModelProfile(profileId: string) {
  return invokeCommand<ModelTestResult>('test_model_profile', { profileId });
}
