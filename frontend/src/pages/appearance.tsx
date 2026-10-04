import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { open } from "@tauri-apps/plugin-dialog";
import { FileCode, Image, Palette, Save } from "lucide-react";
import { toast } from "sonner";

import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import {
    Select,
    SelectContent,
    SelectItem,
    SelectTrigger,
    SelectValue,
} from "@/components/ui/select";
import { Switch } from "@/components/ui/switch";
import { Textarea } from "@/components/ui/textarea";

type ThemeConfig = {
    mode: string;
    background_color: string;
    text_color: string;
    accent_color: string;
    selection_background: string;
    selection_border: string;
    background_image: string;
    custom_css_enabled: boolean;
    custom_css: string;
};

const defaults: ThemeConfig = {
    mode: "system",
    background_color: "",
    text_color: "",
    accent_color: "",
    selection_background: "",
    selection_border: "",
    background_image: "",
    custom_css_enabled: false,
    custom_css: "",
};

const colorValue = (value: string, fallback: string) =>
    /^#[0-9a-fA-F]{6}$/.test(value) ? value : fallback;

export const Appearance = () => {
    const [theme, setTheme] = useState<ThemeConfig>(defaults);
    const [saving, setSaving] = useState(false);

    useEffect(() => {
        invoke<any>("get_config")
            .then((data) => setTheme({ ...defaults, ...(data.theme ?? {}) }))
            .catch(() => toast("テーマ設定を読み込めませんでした"));
    }, []);

    const saveTheme = async () => {
        setSaving(true);
        try {
            const data = await invoke<any>("get_config");
            data.theme = theme;
            await invoke("update_config", { newConfig: data });
            toast("テーマを保存しました");
        } catch {
            toast("テーマの保存に失敗しました");
        } finally {
            setSaving(false);
        }
    };

    const chooseImage = async () => {
        const path = await open({
            multiple: false,
            directory: false,
            filters: [{ name: "画像", extensions: ["png", "jpg", "jpeg", "webp"] }],
        });
        if (typeof path === "string") {
            setTheme((current) => ({ ...current, background_image: path }));
        }
    };

    const background = colorValue(theme.background_color, "#ffffff");
    const text = colorValue(theme.text_color, "#202020");
    const accent = colorValue(theme.selection_background || theme.accent_color, "#d4f0ff");
    const border = colorValue(theme.selection_border || theme.accent_color, "#2cb5ff");

    return (
        <div className="space-y-8">
            <section className="space-y-4">
                <div className="flex items-center justify-between">
                    <h1 className="text-sm font-bold text-foreground">候補ウィンドウのテーマ</h1>
                    <Button onClick={saveTheme} disabled={saving}>
                        <Save />
                        保存
                    </Button>
                </div>

                <div className="flex items-start gap-x-4 pb-2">
                    <div
                        className="w-72 rounded-lg border p-2 shadow-sm"
                        style={{ backgroundColor: background, color: text }}
                    >
                        <ol className="space-y-1">
                            <li className="rounded px-2 py-1" style={{ backgroundColor: accent, outline: `1px solid ${border}` }}>1　変換候補1</li>
                            <li className="px-2 py-1">2　変換候補2</li>
                            <li className="px-2 py-1">3　変換候補3</li>
                        </ol>
                        <div className="mt-2 border-t pt-2 text-xs">azooKey</div>
                    </div>
                </div>

                <div className="flex items-center gap-4 rounded-md border p-4">
                    <Palette />
                    <div className="flex-1">
                        <p className="text-sm font-medium">配色モード</p>
                        <p className="text-xs text-muted-foreground">システム設定、ライト、ダークから選択します</p>
                    </div>
                    <Select value={theme.mode} onValueChange={(mode) => setTheme((current) => ({ ...current, mode }))}>
                        <SelectTrigger className="w-40"><SelectValue /></SelectTrigger>
                        <SelectContent>
                            <SelectItem value="system">システム</SelectItem>
                            <SelectItem value="light">ライト</SelectItem>
                            <SelectItem value="dark">ダーク</SelectItem>
                        </SelectContent>
                    </Select>
                </div>

                {[
                    ["背景色", "background_color"],
                    ["テキスト色", "text_color"],
                    ["アクセント色", "accent_color"],
                    ["選択背景", "selection_background"],
                    ["選択枠", "selection_border"],
                ].map(([label, key]) => (
                    <div key={key} className="flex items-center gap-4 rounded-md border p-4">
                        <Palette />
                        <div className="flex-1">
                            <p className="text-sm font-medium">{label}</p>
                            <p className="text-xs text-muted-foreground">空欄では標準配色を使用します</p>
                        </div>
                        <Input
                            className="w-32"
                            placeholder="#RRGGBB"
                            value={theme[key as keyof ThemeConfig] as string}
                            onChange={(event) => setTheme((current) => ({ ...current, [key]: event.target.value }))}
                            disabled={theme.custom_css_enabled}
                        />
                        <input
                            type="color"
                            value={colorValue(theme[key as keyof ThemeConfig] as string, "#808080")}
                            onChange={(event) => setTheme((current) => ({ ...current, [key]: event.target.value }))}
                            disabled={theme.custom_css_enabled}
                            aria-label={label}
                        />
                    </div>
                ))}

                <div className="flex items-center gap-4 rounded-md border p-4">
                    <Image />
                    <div className="min-w-0 flex-1">
                        <p className="text-sm font-medium">背景画像</p>
                        <p className="truncate text-xs text-muted-foreground">
                            {theme.background_image || "未設定"}
                        </p>
                    </div>
                    {theme.background_image && (
                        <Button variant="secondary" onClick={() => setTheme((current) => ({ ...current, background_image: "" }))}>
                            解除
                        </Button>
                    )}
                    <Button variant="secondary" onClick={chooseImage} disabled={theme.custom_css_enabled}>
                        画像を選択
                    </Button>
                </div>

                <div className="space-y-3 rounded-md border p-4">
                    <div className="flex items-center gap-4">
                        <FileCode />
                        <div className="flex-1">
                            <p className="text-sm font-medium">カスタムCSS</p>
                            <p className="text-xs text-muted-foreground">有効時は候補WebViewにこのCSSを追加します</p>
                        </div>
                        <Switch
                            checked={theme.custom_css_enabled}
                            onCheckedChange={(checked) => setTheme((current) => ({ ...current, custom_css_enabled: checked }))}
                        />
                    </div>
                    <Textarea
                        value={theme.custom_css}
                        onChange={(event) => setTheme((current) => ({ ...current, custom_css: event.target.value }))}
                        disabled={!theme.custom_css_enabled}
                        placeholder={"main { border-radius: 4px; }"}
                        className="min-h-40 font-mono"
                    />
                </div>
            </section>
        </div>
    );
};
