import { invoke } from "@tauri-apps/api/core";
import { toast } from "sonner";

export type UserDictionaryEntry = {
    reading: string;
    word: string;
};

type Saved<T> = {
    value: T;
    warning: string | null;
};

let pending: Promise<unknown> = Promise.resolve();

// Changes are sent one at a time, in the order they were made, so that a
// change that reaches the backend late cannot overwrite a later one (for
// example while typing into a field that is saved on every keystroke).
const save = <T>(command: string, args: Record<string, unknown>): Promise<T> => {
    const result = pending.then(() => invoke<Saved<T>>(command, args));
    pending = result.catch(() => undefined);
    return result.then(({ value, warning }) => {
        if (warning) {
            toast(warning);
        }
        return value;
    });
};

/** The settings as they are in the settings file now. */
export const loadConfig = () => invoke<any>("get_config");

/**
 * Saves only the settings in `patch` (a JSON merge patch: nested objects
 * change only the keys they contain), keeping every other setting as it is in
 * the file, including changes made by other settings windows. Resolves to all
 * saved settings.
 */
export const updateConfig = (patch: Record<string, unknown>) =>
    save<any>("update_config", { patch });

export const addDictionaryEntries = (entries: UserDictionaryEntry[]) =>
    save<{ entries: UserDictionaryEntry[]; added: number }>("add_dictionary_entries", {
        entries,
    });

export const removeDictionaryEntry = (entry: UserDictionaryEntry) =>
    save<UserDictionaryEntry[]>("remove_dictionary_entry", { entry });

/**
 * The fields of a form that the user changed since it was loaded. Saving only
 * these keeps the fields that another window changed in the meantime.
 */
export const changedFields = <T extends Record<string, unknown>>(
    loaded: T,
    edited: T,
): Partial<T> =>
    Object.fromEntries(
        Object.entries(edited).filter(([key, value]) => loaded[key] !== value),
    ) as Partial<T>;
