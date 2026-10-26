import { createContext, useContext, useState, type ReactNode } from "react";
import { api, type Overview } from "../api";

const Context = createContext<{
  device: (id: string) => string;
  session: (deviceId: string, sessionId: string) => string;
  saveDevice: (id: string, value: string) => Promise<void>;
  saveSession: (deviceId: string, sessionId: string, value: string) => Promise<void>;
} | null>(null);

export function RemarksProvider({ snapshot, children }: { snapshot?: Overview["remarks"]; children: ReactNode }) {
  const [devices, setDevices] = useState<Record<string, string>>({});
  const [sessions, setSessions] = useState<Record<string, Record<string, string>>>({});
  return (
    <Context.Provider value={{
      device: (id) => devices[id] ?? snapshot?.[id]?.device ?? "",
      session: (id, sessionId) => sessions[id]?.[sessionId] ?? snapshot?.[id]?.sessions[sessionId] ?? "",
      saveDevice: async (id, value) => {
        const saved = await api.setDeviceRemark(id, value);
        setDevices((current) => ({ ...current, [id]: saved }));
      },
      saveSession: async (id, sessionId, value) => {
        const saved = await api.setSessionRemark(id, sessionId, value);
        setSessions((current) => ({ ...current, [id]: { ...current[id], [sessionId]: saved } }));
      },
    }}>{children}</Context.Provider>
  );
}

export function useRemarks() {
  const value = useContext(Context);
  if (!value) throw new Error("Missing RemarksProvider");
  return value;
}
