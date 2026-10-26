import { ReactNode, useCallback, useEffect, useState } from "react";
import { RefreshCcw, TriangleAlert } from "lucide-react";

import { Button } from "@/components/ui/button";
import { loadConfig } from "@/lib/config";

type State = { kind: "loading" } | { kind: "loaded" } | { kind: "failed"; error: string };

/**
 * Shows the settings pages only when the settings file can be read. The pages
 * would otherwise show the default settings as if they were the user's.
 * Saving is refused by the backend in that case, so the file is never
 * replaced with the defaults.
 */
export const ConfigGate = ({ children }: { children: ReactNode }) => {
    const [state, setState] = useState<State>({ kind: "loading" });

    const check = useCallback(() => {
        setState({ kind: "loading" });
        loadConfig()
            .then(() => setState({ kind: "loaded" }))
            .catch((error) => setState({ kind: "failed", error: String(error) }));
    }, []);

    useEffect(check, [check]);

    if (state.kind === "loading") return null;
    if (state.kind === "loaded") return <>{children}</>;

    return (
        <div className="space-y-4 rounded-md border border-destructive p-4">
            <div className="flex items-start gap-4">
                <TriangleAlert className="mt-0.5 shrink-0 text-destructive" />
                <div className="space-y-2">
                    <p className="text-sm font-medium">設定ファイルを読み込めませんでした</p>
                    <p className="text-xs text-muted-foreground">
                        設定を表示・変更できません。設定ファイルを上書きしないよう、保存も行いません。
                        ファイルを修正してから再読み込みしてください。ファイルを削除すると既定の設定に戻ります。
                    </p>
                    <p className="break-all text-xs">{state.error}</p>
                </div>
            </div>
            <Button type="button" variant="secondary" onClick={check}>
                <RefreshCcw />
                再読み込み
            </Button>
        </div>
    );
};
