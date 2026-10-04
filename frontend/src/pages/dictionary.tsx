import { FormEvent, useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { open, save } from "@tauri-apps/plugin-dialog";
import { BookOpen, FileDown, FileUp, Plus, Trash2 } from "lucide-react";
import { toast } from "sonner";

import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";

type UserDictionaryEntry = {
    reading: string;
    word: string;
};

const entryKey = (entry: UserDictionaryEntry) => `${entry.reading}\u0000${entry.word}`;

const mergeEntries = (
    current: UserDictionaryEntry[],
    imported: UserDictionaryEntry[],
): UserDictionaryEntry[] => {
    const seen = new Set(current.map(entryKey));
    const result = [...current];
    for (const entry of imported) {
        const key = entryKey(entry);
        if (!seen.has(key)) {
            seen.add(key);
            result.push(entry);
        }
    }
    return result;
};

export const Dictionary = () => {
    const [entries, setEntries] = useState<UserDictionaryEntry[]>([]);
    const [reading, setReading] = useState("");
    const [word, setWord] = useState("");
    const [saving, setSaving] = useState(false);

    useEffect(() => {
        invoke<any>("get_config")
            .then((data) => {
                const loaded = data.dictionary?.entries;
                if (Array.isArray(loaded)) {
                    setEntries(
                        loaded.filter(
                            (entry): entry is UserDictionaryEntry =>
                                typeof entry?.reading === "string" &&
                                typeof entry?.word === "string",
                        ),
                    );
                }
            })
            .catch(() => toast("ユーザー辞書の読み込みに失敗しました"));
    }, []);

    const persistEntries = async (nextEntries: UserDictionaryEntry[]) => {
        setSaving(true);
        try {
            const data = await invoke<any>("get_config");
            data.dictionary = { entries: nextEntries };
            await invoke("update_config", { newConfig: data });
            setEntries(nextEntries);
            return true;
        } catch {
            toast("ユーザー辞書の更新に失敗しました");
            return false;
        } finally {
            setSaving(false);
        }
    };

    const addEntry = async (event: FormEvent) => {
        event.preventDefault();

        const normalizedReading = reading.trim();
        const normalizedWord = word.trim();
        if (!normalizedReading || !normalizedWord) {
            toast("読みと単語の両方を入力してください");
            return;
        }

        const entry = { reading: normalizedReading, word: normalizedWord };
        if (entries.some((current) => entryKey(current) === entryKey(entry))) {
            toast("同じ項目がすでに登録されています");
            return;
        }

        if (await persistEntries([...entries, entry])) {
            setReading("");
            setWord("");
            toast("辞書に登録しました");
        }
    };

    const removeEntry = async (index: number) => {
        const nextEntries = entries.filter((_, entryIndex) => entryIndex !== index);
        if (await persistEntries(nextEntries)) {
            toast("辞書から削除しました");
        }
    };

    const importDictionary = async () => {
        const path = await open({
            multiple: false,
            directory: false,
            filters: [
                { name: "ユーザー辞書", extensions: ["json", "tsv", "txt"] },
            ],
        });
        if (typeof path !== "string") return;

        try {
            const imported = await invoke<UserDictionaryEntry[]>(
                "import_dictionary_file",
                { path },
            );
            const merged = mergeEntries(entries, imported);
            const added = merged.length - entries.length;
            if (added === 0) {
                toast("追加できる新しい項目はありませんでした");
                return;
            }
            if (await persistEntries(merged)) {
                toast(`${added}件をインポートしました`);
            }
        } catch (error) {
            toast("辞書のインポートに失敗しました", {
                description: String(error),
            });
        }
    };

    const exportDictionary = async () => {
        const path = await save({
            defaultPath: "azookey-user-dictionary.json",
            filters: [
                { name: "azooKey JSON", extensions: ["json"] },
                { name: "IME TSV", extensions: ["tsv", "txt"] },
            ],
        });
        if (typeof path !== "string") return;

        try {
            await invoke("export_dictionary_file", { path });
            toast("ユーザー辞書を書き出しました");
        } catch (error) {
            toast("辞書のエクスポートに失敗しました", {
                description: String(error),
            });
        }
    };

    return (
        <div className="space-y-8">
            <section className="space-y-2">
                <div className="flex items-center justify-between">
                    <h1 className="text-sm font-bold text-foreground">ユーザー辞書</h1>
                    <div className="flex gap-2">
                        <Button type="button" variant="secondary" onClick={importDictionary} disabled={saving}>
                            <FileUp />
                            インポート
                        </Button>
                        <Button type="button" variant="secondary" onClick={exportDictionary} disabled={saving || entries.length === 0}>
                            <FileDown />
                            エクスポート
                        </Button>
                    </div>
                </div>
                <form onSubmit={addEntry} className="space-y-4 rounded-md border p-4">
                    <div className="flex items-start gap-4">
                        <BookOpen className="mt-2 shrink-0" />
                        <div className="grid flex-1 grid-cols-2 gap-3">
                            <div className="space-y-1">
                                <p className="text-xs text-muted-foreground">読み</p>
                                <Input
                                    value={reading}
                                    onChange={(event) => setReading(event.target.value)}
                                    placeholder="例: あずーきー"
                                    disabled={saving}
                                />
                            </div>
                            <div className="space-y-1">
                                <p className="text-xs text-muted-foreground">単語</p>
                                <Input
                                    value={word}
                                    onChange={(event) => setWord(event.target.value)}
                                    placeholder="例: azooKey"
                                    disabled={saving}
                                />
                            </div>
                        </div>
                        <Button type="submit" disabled={saving}>
                            <Plus />
                            登録
                        </Button>
                    </div>
                </form>
            </section>

            <section className="space-y-2">
                <h2 className="text-sm font-bold text-foreground">
                    登録済みの単語（{entries.length}件）
                </h2>
                {entries.length === 0 ? (
                    <div className="rounded-md border p-6 text-sm text-muted-foreground">
                        登録されている単語はありません。
                    </div>
                ) : (
                    <div className="divide-y rounded-md border">
                        {entries.map((entry, index) => (
                            <div key={`${entryKey(entry)}\u0000${index}`} className="flex items-center gap-4 p-4">
                                <div className="min-w-0 flex-1">
                                    <p className="truncate text-sm font-medium">{entry.word}</p>
                                    <p className="truncate text-xs text-muted-foreground">{entry.reading}</p>
                                </div>
                                <Button
                                    type="button"
                                    variant="destructive"
                                    size="icon"
                                    onClick={() => removeEntry(index)}
                                    disabled={saving}
                                    aria-label={`${entry.word}を削除`}
                                >
                                    <Trash2 />
                                </Button>
                            </div>
                        ))}
                    </div>
                )}
            </section>
        </div>
    );
};
