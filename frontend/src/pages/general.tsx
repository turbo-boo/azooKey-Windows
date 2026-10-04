import { Button } from "@/components/ui/button";
import {
    Select,
    SelectContent,
    SelectItem,
    SelectTrigger,
    SelectValue,
} from "@/components/ui/select";
import { Brain, ExternalLink, RefreshCcw, Trash2 } from "lucide-react";
import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { toast } from "sonner";

type LearningMode = "inputAndOutput" | "onlyOutput" | "nothing";

export const General = () => {
    const [learningMode, setLearningMode] = useState<LearningMode>("inputAndOutput");

    useEffect(() => {
        invoke<any>("get_config")
            .then((data) => {
                const mode = data.learning?.mode;
                if (
                    mode === "inputAndOutput" ||
                    mode === "onlyOutput" ||
                    mode === "nothing"
                ) {
                    setLearningMode(mode);
                }
            })
            .catch(() => {
                toast("設定の読み込みに失敗しました");
            });
    }, []);

    const updateLearningMode = async (mode: LearningMode) => {
        try {
            const data = await invoke<any>("get_config");
            data.learning = { mode };
            await invoke("update_config", { newConfig: data });
            setLearningMode(mode);
        } catch {
            toast("学習設定の更新に失敗しました");
        }
    };

    const resetLearning = async () => {
        if (!window.confirm("学習履歴をリセットします。この操作は取り消せません。")) {
            return;
        }

        try {
            await invoke("reset_learning");
            toast("学習履歴をリセットしました");
        } catch {
            toast("学習履歴のリセットに失敗しました");
        }
    };

    return (
        <div className="space-y-8">
            <section className="space-y-2">
                <h1 className="text-sm font-bold text-foreground">学習機能</h1>
                <div className="flex items-center space-x-4 rounded-md border p-4">
                    <Brain />
                    <div className="flex-1 space-y-1">
                        <p className="text-sm font-medium leading-none">
                            学習の使用
                        </p>
                        <p className="text-xs text-muted-foreground">
                            変換履歴を候補順位へ反映する方法を選択します
                        </p>
                    </div>
                    <Select
                        value={learningMode}
                        onValueChange={(value) => updateLearningMode(value as LearningMode)}
                    >
                        <SelectTrigger className="w-[220px]">
                            <SelectValue />
                        </SelectTrigger>
                        <SelectContent>
                            <SelectItem value="inputAndOutput">
                                学習する（デフォルト）
                            </SelectItem>
                            <SelectItem value="onlyOutput">
                                新たな学習を停止
                            </SelectItem>
                            <SelectItem value="nothing">
                                これまでの学習を反映しない
                            </SelectItem>
                        </SelectContent>
                    </Select>
                </div>
                <div className="flex items-center space-x-4 rounded-md border p-4">
                    <Trash2 />
                    <div className="flex-1 space-y-1">
                        <p className="text-sm font-medium leading-none">
                            学習履歴をリセット
                        </p>
                        <p className="text-xs text-muted-foreground">
                            保存された変換学習をすべて削除します
                        </p>
                    </div>
                    <Button variant="destructive" onClick={resetLearning}>
                        リセット
                    </Button>
                </div>
            </section>

            <section className="space-y-2">
                <h1 className="text-sm font-bold text-foreground">バージョンと更新プログラム</h1>
                <div className="flex items-center space-x-4 rounded-md border p-4">
                    <RefreshCcw />
                    <div className="flex-1 space-y-1">
                        <p className="text-sm font-medium leading-none">
                            v0.1.0-alpha.1
                        </p>
                    </div>
                    <Button variant="secondary">
                        <a
                            href="https://github.com/fkunn1326/azooKey-Windows/releases"
                            className="flex items-center gap-x-2"
                            target="_blank"
                            rel="noopener noreferrer"
                        >
                            <ExternalLink />
                            更新を確認する
                        </a>
                    </Button>
                </div>
            </section>
        </div>
    );
};
