import { useCallback, useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { useToast } from "./useToast";

interface AppImageIntegrationStatusRaw {
  is_appimage: boolean;
  integrated: boolean;
}

export interface UseAppImageIntegrationResult {
  isAppImage: boolean;
  integrated: boolean;
  installing: boolean;
  install: () => Promise<void>;
}

export function useAppImageIntegration(): UseAppImageIntegrationResult {
  const { toast } = useToast();
  const [isAppImage, setIsAppImage] = useState(false);
  const [integrated, setIntegrated] = useState(false);
  const [installing, setInstalling] = useState(false);

  const refresh = useCallback(async () => {
    try {
      const status = await invoke<AppImageIntegrationStatusRaw>(
        "get_appimage_integration_status",
      );
      setIsAppImage(status.is_appimage);
      setIntegrated(status.integrated);
    } catch {
      // Treat a failed status call as not-appimage / not-integrated so the
      // Settings row simply renders nothing rather than erroring.
      setIsAppImage(false);
      setIntegrated(false);
    }
  }, []);

  useEffect(() => {
    void refresh();
  }, [refresh]);

  const install = useCallback(async () => {
    if (integrated || installing) return;
    setInstalling(true);
    try {
      await invoke("integrate_appimage");
      toast("info", "Quill added to your applications menu");
      await refresh();
    } catch (e) {
      toast("error", `Couldn't add to applications menu: ${e}`);
    } finally {
      setInstalling(false);
    }
  }, [toast, refresh, integrated, installing]);

  return { isAppImage, integrated, installing, install };
}
