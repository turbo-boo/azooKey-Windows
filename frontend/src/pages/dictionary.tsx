import { FormEvent, useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { open, save } from "@tauri-apps/plugin-dialog";
import { BookOpen, FileDown, FileUp, Plus, Trash2 } from "lucide-react";
import { toast } from "sonner";

import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import {
    addDictionaryEntries,
    loadConfig,
    removeDictionaryEntry,
    type UserDictionaryEntry,
} from "@/lib/config";

const entryKey = (entry: UserDictionaryEntry) => `${entry.reading}\u0000${entry.word}`;

export const Dictionary = () => {
    const [entries, setEntries] = useState<UserDictionaryEntry[]>([]);
    const [reading, setReading] = useState("");
    const [word, setWord] = useState("");
    const [saving, setSaving] = useState(false);

    useEffect(() => {
        loadConfig()
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
            .catch((error) =>
                toast("ユーザー辞書の読み込みに失敗しました", { description: String(error) }),
            );
    }, []);

    // The changes are applied to the dictionary as it is in the settings file
    // now, so entries that another settings window added or removed in the
    // meantime are kept as they are; the list then shows the saved result.
    const changeEntries = async <T,>(change: () => Promise<T>) => {
        setSaving(true);
        try {
            return await change();
        } catch (error) {
            toast("ユーザー辞書の更新に失敗しました", { description: String(error) });
            return null;
        } finally {
            setSaving(false);
        }
    };

    const addEntries = async (added: UserDictionaryEntry[]) => {
        const result = await changeEntries(() => addDictionaryEntries(added));
        if (result) setEntries(result.entries);
        return result;
    };

    const addEntry = async (event: FormEvent) => {
        event.preventDefault();

        const normalizedReading = reading.trim();
        const normalizedWord = word.trim();
        if (!normalizedReading || !normalizedWord) {
            toast("読みと単語の両方を入力してください");
            return;
        }

        const result = await addEntries([
            { reading: normalizedReading, word: normalizedWord },
        ]);
        if (!result) return;
        if (result.added === 0) {
            toast("同じ項目がすでに登録されています");
            return;
        }
        setReading("");
        setWord("");
        toast("辞書に登録しました");
    };

    const removeEntry = async (entry: UserDictionaryEntry) => {
        const remaining = await changeEntries(() => removeDictionaryEntry(entry));
        if (remaining) {
            setEntries(remaining);
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
            const result = await addEntries(imported);
            if (!result) return;
            if (result.added === 0) {
                toast("追加できる新しい項目はありませんでした");
                return;
            }
            toast(`${result.added}件をインポートしました`);
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
                                    onClick={() => removeEntry(entry)}
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
