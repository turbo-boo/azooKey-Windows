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
import { useEffect, useState, type KeyboardEvent } from "react";
import { invoke } from "@tauri-apps/api/core";
import { toast } from "sonner";

type LearningMode = "inputAndOutput" | "onlyOutput" | "nothing";

type ShortcutBinding = {
    key_code: number;
    ctrl: boolean;
    alt: boolean;
    shift: boolean;
};

const DEFAULT_SHORTCUTS: ShortcutBinding[] = [
    { key_code: 0x20, ctrl: true, alt: false, shift: false },
    { key_code: 0xc0, ctrl: false, alt: true, shift: false },
];

const shortcutKeyName = (keyCode: number) => {
    if (keyCode >= 0x41 && keyCode <= 0x5a) return String.fromCharCode(keyCode);
    if (keyCode >= 0x30 && keyCode <= 0x39) return String.fromCharCode(keyCode);
    if (keyCode >= 0x70 && keyCode <= 0x7b) return `F${keyCode - 0x6f}`;

    const names: Record<number, string> = {
        0x09: "Tab",
        0x0d: "Enter",
        0x20: "Space",
        0x25: "←",
        0x26: "↑",
        0x27: "→",
        0x28: "↓",
        0xba: ";",
        0xbb: "=",
        0xbc: ",",
        0xbd: "-",
        0xbe: ".",
        0xbf: "/",
        0xc0: "`",
        0xdb: "[",
        0xdc: "\\",
        0xdd: "]",
        0xde: "'",
    };
    return names[keyCode] ?? `VK 0x${keyCode.toString(16).toUpperCase().padStart(2, "0")}`;
};

const shortcutLabel = (binding?: ShortcutBinding) => {
    if (!binding) return "なし（クリックして設定）";
    return [
        binding.ctrl ? "Ctrl" : "",
        binding.alt ? "Alt" : "",
        binding.shift ? "Shift" : "",
        shortcutKeyName(binding.key_code),
    ].filter(Boolean).join(" + ");
};

const bindingFromKeyboardEvent = (event: KeyboardEvent<HTMLInputElement>): ShortcutBinding | null => {
    if (event.metaKey) return null;
    const modifierOnly = [
        "ControlLeft",
        "ControlRight",
        "AltLeft",
        "AltRight",
        "ShiftLeft",
        "ShiftRight",
        "MetaLeft",
        "MetaRight",
    ];
    if (modifierOnly.includes(event.code)) return null;

    const keyCode = event.keyCode;
    if (
        !Number.isInteger(keyCode) ||
        keyCode <= 0 ||
        keyCode > 0xff ||
        (!event.ctrlKey && !event.altKey && !event.shiftKey)
    ) {
        return null;
    }

    return {
        key_code: keyCode,
        ctrl: event.ctrlKey,
        alt: event.altKey,
        shift: event.shiftKey,
    };
};

const sameShortcut = (a: ShortcutBinding, b: ShortcutBinding) =>
    a.key_code === b.key_code &&
    a.ctrl === b.ctrl &&
    a.alt === b.alt &&
    a.shift === b.shift;

export const General = () => {
    const [learningMode, setLearningMode] = useState<LearningMode>("inputAndOutput");
    const [liveConversionEnabled, setLiveConversionEnabled] = useState(true);
    const [shortcutBindings, setShortcutBindings] = useState<ShortcutBinding[]>(DEFAULT_SHORTCUTS);
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
                setLiveConversionEnabled(data.live_conversion?.enabled ?? true);
                const shortcuts = data.shortcuts ?? {};
                if (Array.isArray(shortcuts.toggle_bindings)) {
                    setShortcutBindings(
                        shortcuts.toggle_bindings
                            .filter((binding: ShortcutBinding) =>
                                binding &&
                                Number.isInteger(binding.key_code) &&
                                binding.key_code > 0 &&
                                binding.key_code <= 0xff &&
                                (binding.ctrl || binding.alt || binding.shift)
                            )
                            .slice(0, 2)
                    );
                } else {
                    const legacy: ShortcutBinding[] = [];
                    if (shortcuts.ctrl_space_toggle ?? true) legacy.push(DEFAULT_SHORTCUTS[0]);
                    if (shortcuts.alt_backquote_toggle ?? true) legacy.push(DEFAULT_SHORTCUTS[1]);
                    setShortcutBindings(legacy);
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

    const updateLiveConversion = async (enabled: boolean) => {
        if (await updateGeneralConfig((data) => {
            data.live_conversion = { enabled };
        })) {
            setLiveConversionEnabled(enabled);
        }
    };

    const saveShortcutBindings = async (bindings: ShortcutBinding[]) => {
        const next = bindings.slice(0, 2);
        if (await updateGeneralConfig((data) => {
            data.shortcuts = data.shortcuts ?? {};
            data.shortcuts.toggle_bindings = next;
        })) {
            setShortcutBindings(next);
        }
    };

    const captureShortcut = async (
        index: number,
        event: KeyboardEvent<HTMLInputElement>,
    ) => {
        event.preventDefault();
        event.stopPropagation();

        if (event.key === "Escape") {
            event.currentTarget.blur();
            return;
        }

        const binding = bindingFromKeyboardEvent(event);
        if (!binding) {
            toast("Ctrl / Alt / Shift のいずれかを含むショートカットを入力してください");
            return;
        }

        if (
            shortcutBindings.some(
                (item, itemIndex) => itemIndex !== index && sameShortcut(item, binding),
            )
        ) {
            toast("同じショートカットは2枠に登録できません");
            return;
        }

        const next = [...shortcutBindings];
        next[index] = binding;
        await saveShortcutBindings(next.filter(Boolean));
        event.currentTarget.blur();
    };

    const clearShortcut = async (index: number) => {
        await saveShortcutBindings(
            shortcutBindings.filter((_, itemIndex) => itemIndex !== index),
        );
    };

    const resetShortcuts = async () => {
        await saveShortcutBindings(DEFAULT_SHORTCUTS);
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
                <div className="flex items-center justify-between">
                    <h1 className="text-sm font-bold text-foreground">
                        入力モード切替ショートカット
                    </h1>
                    <Button variant="secondary" size="sm" onClick={resetShortcuts}>
                        既定に戻す
                    </Button>
                </div>
                <p className="text-xs text-muted-foreground">
                    2枠まで設定できます。入力欄を選択して、Ctrl / Alt / Shiftを含むキー操作を押してください。
                </p>
                {[0, 1].map((index) => (
                    <div
                        key={index}
                        className="flex items-center space-x-4 rounded-md border p-4"
                    >
                        <Keyboard />
                        <div className="flex-1 space-y-2">
                            <p className="text-sm font-medium leading-none">
                                ショートカット {index + 1}
                            </p>
                            <Input
                                readOnly
                                value={shortcutLabel(shortcutBindings[index])}
                                onKeyDown={(event) => captureShortcut(index, event)}
                                onFocus={(event) => event.currentTarget.select()}
                                aria-label={`入力モード切替ショートカット ${index + 1}`}
                            />
                        </div>
                        <Button
                            variant="secondary"
                            disabled={!shortcutBindings[index]}
                            onClick={() => clearShortcut(index)}
                        >
                            解除
                        </Button>
                    </div>
                ))}
                <p className="text-xs text-muted-foreground">
                    VK_IME_ON / VK_IME_OFF と半角/全角キーはこの設定とは別に利用できます。
                </p>
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
