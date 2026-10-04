import { FormEvent, useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { BookOpen, Plus, Trash2 } from "lucide-react";
import { toast } from "sonner";

import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";

type UserDictionaryEntry = {
    reading: string;
    word: string;
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
            .catch(() => {
                toast("ユーザー辞書の読み込みに失敗しました");
            });
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

        if (
            entries.some(
                (entry) =>
                    entry.reading === normalizedReading &&
                    entry.word === normalizedWord,
            )
        ) {
            toast("同じ項目がすでに登録されています");
            return;
        }

        const nextEntries = [
            ...entries,
            { reading: normalizedReading, word: normalizedWord },
        ];

        if (await persistEntries(nextEntries)) {
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

    return (
        <div className="space-y-8">
            <section className="space-y-2">
                <h1 className="text-sm font-bold text-foreground">ユーザー辞書</h1>
                <form
                    onSubmit={addEntry}
                    className="space-y-4 rounded-md border p-4"
                >
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
                    登録済みの単語
                </h2>
                {entries.length === 0 ? (
                    <div className="rounded-md border p-6 text-sm text-muted-foreground">
                        登録されている単語はありません。
                    </div>
                ) : (
                    <div className="divide-y rounded-md border">
                        {entries.map((entry, index) => (
                            <div
                                key={`${entry.reading}\u0000${entry.word}\u0000${index}`}
                                className="flex items-center gap-4 p-4"
                            >
                                <div className="min-w-0 flex-1">
                                    <p className="truncate text-sm font-medium">
                                        {entry.word}
                                    </p>
                                    <p className="truncate text-xs text-muted-foreground">
                                        {entry.reading}
                                    </p>
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
