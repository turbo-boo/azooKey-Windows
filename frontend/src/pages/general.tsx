import { Button } from "@/components/ui/button";
import {
    Select,
    SelectContent,
    SelectItem,
    SelectTrigger,
    SelectValue,
} from "@/components/ui/select";
import { Brain, ExternalLink, RefreshCcw, Sparkles, Trash2 } from "lucide-react";
import { Switch } from "@/components/ui/switch";
import { Input } from "@/components/ui/input";
import { Textarea } from "@/components/ui/textarea";
import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { toast } from "sonner";

type LearningMode = "inputAndOutput" | "onlyOutput" | "nothing";

export const General = () => {
    const [learningMode, setLearningMode] = useState<LearningMode>("inputAndOutput");
    const [predictionEnabled, setPredictionEnabled] = useState(false);
    const [magic, setMagic] = useState({
        enabled: false,
        endpoint: "https://api.openai.com/v1/chat/completions",
        model: "gpt-4o-mini",
        api_key: "",
        instruction: "自然で読みやすい別表現に変換してください。",
        timeout_seconds: 10,
    });

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
                setPredictionEnabled(data.prediction?.enabled ?? false);
                setMagic((current) => ({ ...current, ...(data.magic_conversion ?? {}) }));
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

    const updateGeneralConfig = async (updater: (config: any) => void) => {
        try {
            const data = await invoke<any>("get_config");
            updater(data);
            await invoke("update_config", { newConfig: data });
            return true;
        } catch {
            toast("設定の更新に失敗しました");
            return false;
        }
    };

    const updatePrediction = async (enabled: boolean) => {
        if (await updateGeneralConfig((data) => {
            data.prediction.enabled = enabled;
        })) {
            setPredictionEnabled(enabled);
        }
    };

    const saveMagic = async () => {
        if (!magic.endpoint.trim() || !magic.model.trim()) {
            toast("APIエンドポイントとモデル名を入力してください");
            return;
        }
        if (await updateGeneralConfig((data) => {
            data.magic_conversion = {
                ...magic,
                timeout_seconds: Math.min(60, Math.max(1, Number(magic.timeout_seconds) || 10)),
            };
        })) {
            toast("いい感じ変換の設定を保存しました");
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
                <h1 className="text-sm font-bold text-foreground">予測変換</h1>
                <div className="flex items-center space-x-4 rounded-md border p-4">
                    <Brain />
                    <div className="flex-1 space-y-1">
                        <p className="text-sm font-medium leading-none">日本語の予測候補</p>
                        <p className="text-xs text-muted-foreground">
                            変換確定後にKanaKanjiConverterの次入力候補を表示します
                        </p>
                    </div>
                    <Switch checked={predictionEnabled} onCheckedChange={updatePrediction} />
                </div>
            </section>

            <section className="space-y-3">
                <h1 className="text-sm font-bold text-foreground">いい感じ変換</h1>
                <div className="space-y-4 rounded-md border p-4">
                    <div className="flex items-center gap-4">
                        <Sparkles />
                        <div className="flex-1">
                            <p className="text-sm font-medium">F5でAI変換候補を生成</p>
                            <p className="text-xs text-muted-foreground">
                                通常入力中は通信せず、F5を押した時だけOpenAI互換APIへ送信します
                            </p>
                        </div>
                        <Switch
                            checked={magic.enabled}
                            onCheckedChange={(enabled) => setMagic((current) => ({ ...current, enabled }))}
                        />
                    </div>
                    <Input
                        value={magic.endpoint}
                        onChange={(event) => setMagic((current) => ({ ...current, endpoint: event.target.value }))}
                        placeholder="API endpoint"
                    />
                    <div className="grid grid-cols-2 gap-3">
                        <Input
                            value={magic.model}
                            onChange={(event) => setMagic((current) => ({ ...current, model: event.target.value }))}
                            placeholder="model"
                        />
                        <Input
                            type="password"
                            value={magic.api_key}
                            onChange={(event) => setMagic((current) => ({ ...current, api_key: event.target.value }))}
                            placeholder="API key（ローカルAPIでは空欄可）"
                        />
                    </div>
                    <Textarea
                        value={magic.instruction}
                        onChange={(event) => setMagic((current) => ({ ...current, instruction: event.target.value }))}
                        placeholder="変換指示"
                    />
                    <div className="flex items-center justify-between">
                        <label className="flex items-center gap-2 text-xs text-muted-foreground">
                            タイムアウト（秒）
                            <Input
                                type="number"
                                min={1}
                                max={60}
                                className="w-20"
                                value={magic.timeout_seconds}
                                onChange={(event) => setMagic((current) => ({ ...current, timeout_seconds: Number(event.target.value) }))}
                            />
                        </label>
                        <Button onClick={saveMagic}>保存</Button>
                    </div>
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
