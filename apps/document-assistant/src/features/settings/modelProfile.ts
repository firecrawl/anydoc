import { invokeCommand } from '../../lib/desktop/api';

export type ModelRole = 'vision' | 'text';

export interface ModelProfile {
  id: string;
  role: ModelRole;
  baseUrl: string;
  model: string;
  supportsVision: boolean;
  timeoutSecs: number;
  maxConcurrency: number;
}

export interface ModelProfileStatus extends ModelProfile {
  hasApiKey: boolean;
  capabilityTested: boolean;
}

export function saveModelProfile(profile: ModelProfile) {
  return invokeCommand<void>('save_model_profile', { profile });
}

export function setModelApiKey(profileId: string, apiKey: string) {
  return invokeCommand<void>('set_api_key', { profileId, apiKey });
}

export function listModelProfiles() {
  return invokeCommand<ModelProfileStatus[]>('list_model_profiles');
}
