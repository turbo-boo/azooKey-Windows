import { Textarea } from "@/components/ui/textarea";
import { Input } from "@/components/ui/input";
import { Button } from "@/components/ui/button";
import { open } from "@tauri-apps/plugin-dialog";
import { Switch } from "@/components/ui/switch";
import { Bot, User, Cpu, BrainCircuit, FolderOpen } from "lucide-react";
import {
    Select,
    SelectContent,
    SelectItem,
    SelectTrigger,
    SelectValue,
} from "@/components/ui/select"
import { useEffect, useState } from "react";
import { toast } from "sonner"
import { invoke } from '@tauri-apps/api/core';
import {
    Tooltip,
    TooltipContent,
    TooltipProvider,
    TooltipTrigger,
} from "@/components/ui/tooltip"

const ToolTipSelectItem = ({
    name,
    value,
    disabled,
    tooltip
}: {
    name: string;
    value: string;
    disabled: boolean;
    tooltip: string;
}) => {
    return (
        <TooltipProvider>
            <Tooltip>
                <TooltipTrigger>
                    <SelectItem value={value} disabled={disabled}>
                        {name}
                    </SelectItem>
                </TooltipTrigger>
                {disabled && <TooltipContent side="left">
                    {tooltip}
                </TooltipContent>}
            </Tooltip>
        </TooltipProvider>
    )
}

export const Zenzai = () => {
    const [value, setValue] = useState({
        enable: false,
        profile: "",
        backend: "",
    });

    const [capability, setCapability] = useState({
        cpu: true,
        cuda: false,
        vulkan: false,
    });

    const [personalization, setPersonalization] = useState({
        enabled: false,
        tuner_directory: "",
        alpha: 0.5,
    });

    // Load config on component mount
    useEffect(() => {
        invoke<any>("get_config")
            .then((data) => {
                const zenzai = data.zenzai;
                setValue({
                    enable: zenzai.enable,
                    profile: zenzai.profile,
                    backend: zenzai.backend,
                });
                setPersonalization({
                    enabled: data.personalization?.enabled ?? false,
                    tuner_directory: data.personalization?.tuner_directory ?? "",
                    alpha: data.personalization?.alpha ?? 0.5,
                });
            })
            .catch(() => {
                // Keep default values if config fetch fails
            });

        invoke("check_capability").then((capability: any) => {
            setCapability({
                cpu: capability["cpu"],
                cuda: capability["cuda"],
                vulkan: capability["vulkan"],
            });
        })
    }, []);

    const updateConfig = async (updater: (config: any) => void) => {
        try {
            const data = await invoke<any>("get_config");
            updater(data);
            await invoke("update_config", { newConfig: data });
            return data;
        } catch (error) {
            toast("設定の更新に失敗しました");
            return null;
        }
    };

    const handleZenzaiChange = async () => {
        const data = await updateConfig((data) => {
            data.zenzai.enable = !value.enable;
        });
        
        if (data) {
            setValue((prev) => ({ ...prev, enable: data.zenzai.enable }));
        }
    };

    const handleProfileChange = (event: React.ChangeEvent<HTMLTextAreaElement>) => {
        const newProfile = event.target.value;
        setValue((prev) => ({ ...prev, profile: newProfile }));
        
        updateConfig((data) => {
            data.zenzai.profile = newProfile;
        });
    };

    const chooseTunerDirectory = async () => {
        const path = await open({ directory: true, multiple: false });
        if (typeof path !== "string") return;

        try {
            await invoke("validate_tuner_model", { path });
            const data = await updateConfig((data) => {
                data.personalization.tuner_directory = path;
            });
            if (data) {
                setPersonalization((prev) => ({ ...prev, tuner_directory: path }));
                toast("Tunerモデルを確認しました");
            }
        } catch (error) {
            toast("Tunerモデルを利用できません", { description: String(error) });
        }
    };

    const setPersonalizationEnabled = async (enabled: boolean) => {
        if (enabled && !personalization.tuner_directory) {
            toast("先にTunerモデルのフォルダーを指定してください");
            return;
        }
        if (enabled) {
            try {
                await invoke("validate_tuner_model", { path: personalization.tuner_directory });
            } catch (error) {
                toast("Tunerモデルを利用できません", { description: String(error) });
                return;
            }
        }

        const data = await updateConfig((data) => {
            data.personalization.enabled = enabled;
        });
        if (data) setPersonalization((prev) => ({ ...prev, enabled }));
    };

    const setPersonalizationAlpha = async (alpha: number) => {
        const normalized = Math.min(1, Math.max(0, alpha));
        setPersonalization((prev) => ({ ...prev, alpha: normalized }));
        await updateConfig((data) => {
            data.personalization.alpha = normalized;
        });
    };

    const handleBackendChange = async (backend: string) => {
        const data = await updateConfig((data) => {
            data.zenzai.backend = backend;
        });
        
        if (data) {
            setValue((prev) => ({ ...prev, backend }));
            toast("バックエンドが変更されました", {
                description: "変更を適用するには、PCを再起動してください",
                duration: 10000,
            });
        }
    };

    return (
        <div className="space-y-8">
            <section className="space-y-2">
                <h1 className="text-sm font-bold text-foreground">Zenzai</h1>
                <div className="flex items-center space-x-4 rounded-md border p-4">
                    <Bot />
                    <div className="flex-1 space-y-1">
                        <p className="text-sm font-medium leading-none">
                            Zenzaiを有効化
                        </p>
                        <p className="text-xs text-muted-foreground">
                            Zenzaiを有効にして、変換精度を向上させます
                        </p>
                    </div>
                    <Switch checked={value.enable} onCheckedChange={handleZenzaiChange} />
                </div>
                <div className="space-y-4 rounded-md border p-4">
                    <div className="flex items-center space-x-4 ">
                        <User />
                        <div className="flex-1 space-y-1">
                            <p className="text-sm font-medium leading-none">
                                変換プロファイル
                            </p>
                            <p className="text-xs text-muted-foreground">
                                Zenzaiで利用されるユーザー情報を設定します
                            </p>
                        </div>
                    </div>
                    <Textarea placeholder="例）山田太郎、数学科の学生。" value={value.profile} disabled={!value.enable} onChange={handleProfileChange} />
                </div>
                <div className="space-y-4 rounded-md border p-4">
                    <div className="flex items-center gap-4">
                        <BrainCircuit />
                        <div className="flex-1">
                            <p className="text-sm font-medium">個人最適化システム</p>
                            <p className="text-xs text-muted-foreground">
                                Tunerが生成した original_*.marisa と lm_*.marisa をZenzaiに読み込みます
                            </p>
                        </div>
                        <Switch
                            checked={personalization.enabled}
                            onCheckedChange={setPersonalizationEnabled}
                            disabled={!value.enable}
                        />
                    </div>
                    <div className="flex items-center gap-2">
                        <Input
                            value={personalization.tuner_directory}
                            readOnly
                            placeholder="Tunerのp13n_v1フォルダー"
                            className="flex-1"
                        />
                        <Button type="button" variant="secondary" onClick={chooseTunerDirectory}>
                            <FolderOpen />
                            選択
                        </Button>
                    </div>
                    <div className="flex items-center gap-4">
                        <span className="text-xs text-muted-foreground">適用強度</span>
                        <input
                            type="range"
                            min="0"
                            max="1"
                            step="0.05"
                            value={personalization.alpha}
                            onChange={(event) => setPersonalizationAlpha(Number(event.target.value))}
                            disabled={!personalization.enabled}
                            className="flex-1"
                        />
                        <span className="w-10 text-right text-xs">{personalization.alpha.toFixed(2)}</span>
                    </div>
                </div>

                <div className="flex items-center space-x-4 rounded-md border p-4">
                    <Cpu />
                    <div className="flex-1 space-y-1">
                        <p className="text-sm font-medium leading-none">
                            バックエンド
                        </p>
                        <p className="text-xs text-muted-foreground">
                            Zenzaiを利用するバックエンドを選択します
                        </p>
                    </div>
                    <Select disabled={!value.enable} value={value.backend} onValueChange={handleBackendChange}>
                        <SelectTrigger className="w-48">
                            <SelectValue placeholder="バックエンドを選択" />
                        </SelectTrigger>
                        <SelectContent className="flex flex-col">
                            <ToolTipSelectItem name="CPU (非推奨)" value="cpu" disabled={!capability.cpu} tooltip="" />
                            <ToolTipSelectItem name="CUDA (NVIDIA GPU)" value="cuda" disabled={!capability.cuda} tooltip="CUDA Toolkit 12をインストールする必要があります" />
                            <ToolTipSelectItem name="Vulkan" value="vulkan" disabled={!capability.vulkan} tooltip="お使いのPCはVulkanに対応していません" />
                        </SelectContent>
                    </Select>
                </div>
            </section>
        </div>
    )
}