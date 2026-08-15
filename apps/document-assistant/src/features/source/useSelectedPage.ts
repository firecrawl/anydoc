import { createContext, useContext } from 'react';

export interface SelectedPageState {
  pageNumber: number;
  selectPage: (pageNumber: number) => void;
}

export const SelectedPageContext = createContext<SelectedPageState | null>(null);

export function useSelectedPage() {
  const value = useContext(SelectedPageContext);
  if (!value) throw new Error('useSelectedPage must be used inside SelectedPageContext.Provider');
  return value;
}
