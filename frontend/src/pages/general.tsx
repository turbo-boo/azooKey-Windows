import { Button } from "@/components/ui/button";
import {
    Select,
    SelectContent,
    SelectItem,
    SelectTrigger,
    SelectValue,
} from "@/components/ui/select";
import { Brain, ExternalLink, Keyboard, RefreshCcw, Sparkles, Trash2 } from "lucide-react";
import { Switch } from "@/components/ui/switch";
import { Input } from "@/components/ui/input";
import { Textarea } from "@/components/ui/textarea";
import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { toast } from "sonner";
import { changedFields, loadConfig, updateConfig } from "@/lib/config";

type LearningMode = "inputAndOutput" | "onlyOutput" | "nothing";

const defaultMagic = {
    enabled: false,
    endpoint: "https://api.openai.com/v1/chat/completions",
    model: "gpt-4o-mini",
    api_key: "",
    instruction: "自然で読みやすい別表現に変換してください。",
    timeout_seconds: 10,
};

export const General = () => {
    const [learningMode, setLearningMode] = useState<LearningMode>("inputAndOutput");
    const [liveConversionEnabled, setLiveConversionEnabled] = useState(true);
    const [shortcutValue, setShortcutValue] = useState({
        ctrlSpaceToggle: true,
        altBackquoteToggle: true,
    });
    const [predictionEnabled, setPredictionEnabled] = useState(false);
    const [magic, setMagic] = useState(defaultMagic);
    // The magic conversion settings as last loaded or saved, to save only the
    // fields edited in this window.
    const [loadedMagic, setLoadedMagic] = useState(defaultMagic);

    const showMagic = (data: any) => {
        const loaded = { ...defaultMagic, ...(data.magic_conversion ?? {}) };
        setMagic(loaded);
        setLoadedMagic(loaded);
    };

    useEffect(() => {
        loadConfig()
            .then((data) => {
                const mode = data.learning?.mode;
                if (
                    mode === "inputAndOutput" ||
                    mode === "onlyOutput" ||
                    mode === "nothing"
                ) {
                    setLearningMode(mode);
                }
                setLiveConversionEnabled(data.live_conversion?.enabled ?? true);
                const shortcuts = data.shortcuts ?? {};
                setShortcutValue({
                    ctrlSpaceToggle: shortcuts.ctrl_space_toggle ?? true,
                    altBackquoteToggle: shortcuts.alt_backquote_toggle ?? true,
                });
                setPredictionEnabled(data.prediction?.enabled ?? false);
                showMagic(data);
            })
            .catch((error) => {
                toast("設定の読み込みに失敗しました", { description: String(error) });
            });
    }, []);

    const updateLearningMode = async (mode: LearningMode) => {
        try {
            await updateConfig({ learning: { mode } });
            setLearningMode(mode);
        } catch (error) {
            toast("学習設定の更新に失敗しました", { description: String(error) });
        }
    };

    const updateGeneralConfig = async (patch: Record<string, unknown>) => {
        try {
            return await updateConfig(patch);
        } catch (error) {
            toast("設定の更新に失敗しました", { description: String(error) });
            return null;
        }
    };

    const updateLiveConversion = async (enabled: boolean) => {
        if (await updateGeneralConfig({ live_conversion: { enabled } })) {
            setLiveConversionEnabled(enabled);
        }
    };

    const updateShortcut = async (
        key: "ctrl_space_toggle" | "alt_backquote_toggle",
        enabled: boolean,
    ) => {
        if (await updateGeneralConfig({ shortcuts: { [key]: enabled } })) {
            if (key === "ctrl_space_toggle") {
                setShortcutValue((current) => ({ ...current, ctrlSpaceToggle: enabled }));
            } else {
                setShortcutValue((current) => ({ ...current, altBackquoteToggle: enabled }));
            }
        }
    };

    const updatePrediction = async (enabled: boolean) => {
        if (await updateGeneralConfig({ prediction: { enabled } })) {
            setPredictionEnabled(enabled);
        }
    };

    const saveMagic = async () => {
        if (!magic.endpoint.trim() || !magic.model.trim()) {
            toast("APIエンドポイントとモデル名を入力してください");
            return;
        }
        const edited = {
            ...magic,
            // The backend stores whole seconds; a fraction would fail the save.
            timeout_seconds: Math.min(60, Math.max(1, Math.round(Number(magic.timeout_seconds)) || 10)),
        };
        const data = await updateGeneralConfig({
            magic_conversion: changedFields(loadedMagic, edited),
        });
        if (data) {
            showMagic(data);
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
        } catch (error) {
            toast("学習履歴のリセットに失敗しました", { description: String(error) });
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
                <h1 className="text-sm font-bold text-foreground">ライブ変換</h1>
                <div className="flex items-center space-x-4 rounded-md border p-4">
                    <Brain />
                    <div className="flex-1 space-y-1">
                        <p className="text-sm font-medium leading-none">入力中に自動変換</p>
                        <p className="text-xs text-muted-foreground">
                            無効時は入力中の変換計算を行わず、SpaceまたはTabで変換します
                        </p>
                    </div>
                    <Switch checked={liveConversionEnabled} onCheckedChange={updateLiveConversion} />
                </div>
            </section>

            <section className="space-y-2">
                <h1 className="text-sm font-bold text-foreground">入力モード切替ショートカット</h1>
                <div className="flex items-center space-x-4 rounded-md border p-4">
                    <Keyboard />
                    <div className="flex-1 space-y-1">
                        <p className="text-sm font-medium leading-none">Ctrl + Space</p>
                        <p className="text-xs text-muted-foreground">
                            US配列でも英数/かなを切り替えられるようにします
                        </p>
                    </div>
                    <Switch
                        checked={shortcutValue.ctrlSpaceToggle}
                        onCheckedChange={(enabled) => updateShortcut("ctrl_space_toggle", enabled)}
                    />
                </div>
                <div className="flex items-center space-x-4 rounded-md border p-4">
                    <Keyboard />
                    <div className="flex-1 space-y-1">
                        <p className="text-sm font-medium leading-none">Alt + `</p>
                        <p className="text-xs text-muted-foreground">
                            US配列の一般的なIME切替ショートカットを有効にします
                        </p>
                    </div>
                    <Switch
                        checked={shortcutValue.altBackquoteToggle}
                        onCheckedChange={(enabled) => updateShortcut("alt_backquote_toggle", enabled)}
                    />
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
                                step={1}
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
