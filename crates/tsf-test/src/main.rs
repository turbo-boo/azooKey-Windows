//! Types into the azooKey IME through TSF, the way an application receives
//! keys, and checks what the application's document shows.
//!
//! Run it on a disposable Windows machine (CI) after the smoke test: the
//! IME's DLL must be registered (regsvr32) and its converter server running
//! in this session. It activates the text service in this process only,
//! gives it a document of its own (an in-memory text store) and feeds keys
//! to the TSF keystroke manager. Its window takes the keyboard focus, so
//! the desktop must not be in use. A panic in the IME aborts this process,
//! so a crash fails the run as well.
//!
//! `--expect-dll <path>` fails the run unless the text service was loaded
//! from that file, so it tests the build it is meant to.

mod store;

use std::{
    path::{Path, PathBuf},
    process::ExitCode,
    time::{Duration, Instant},
};

use anyhow::{bail, ensure, Context as _, Result};
use windows::{
    core::{w, Interface, GUID, HSTRING},
    Win32::{
        Foundation::{HWND, LPARAM, LRESULT, WPARAM},
        System::{
            Com::{CoCreateInstance, CoInitializeEx, CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED},
            LibraryLoader::{GetModuleFileNameW, GetModuleHandleW},
        },
        UI::{
            Input::KeyboardAndMouse::{GetFocus, MapVirtualKeyW, SetFocus, HKL, MAPVK_VK_TO_VSC},
            TextServices::{
                CLSID_TF_InputProcessorProfiles, CLSID_TF_ThreadMgr, ITextStoreACP, ITfContext,
                ITfContextComposition, ITfDocumentMgr, ITfInputProcessorProfileMgr,
                ITfKeystrokeMgr, ITfThreadMgr, GUID_TFCAT_TIP_KEYBOARD, TF_INPUTPROCESSORPROFILE,
                TF_IPPMF_DONTCARECURRENTINPUTLANGUAGE, TF_IPPMF_FORPROCESS,
                TF_PROFILETYPE_INPUTPROCESSOR,
            },
            WindowsAndMessaging::{
                CreateWindowExW, DefWindowProcW, DispatchMessageW, PeekMessageW, RegisterClassW,
                SetForegroundWindow, ShowWindow, TranslateMessage, MSG, PM_REMOVE, SW_SHOW,
                WINDOW_EX_STYLE, WNDCLASSW, WS_OVERLAPPEDWINDOW,
            },
        },
    },
};

use store::TextStore;

// crates/client/src/globals.rs
const CLSID_AZOOKEY: GUID = GUID::from_u128(0xffdefe79_2fc2_11ef_b16b_94e70b2c378c);
const PROFILE_AZOOKEY: GUID = GUID::from_u128(0xffdefe7a_2fc2_11ef_b16b_94e70b2c378c);
const LANGID_JAPANESE: u16 = 0x0411;

const VK_BACK: u16 = 0x08;
const VK_RETURN: u16 = 0x0D;
const VK_ESCAPE: u16 = 0x1B;
const VK_SPACE: u16 = 0x20;
const VK_UP: u16 = 0x26;
const VK_DOWN: u16 = 0x28;
const VK_F6: u16 = 0x75;
const VK_F7: u16 = 0x76;
const VK_F8: u16 = 0x77;
const VK_F9: u16 = 0x78;
const VK_F10: u16 = 0x79;
const VK_IME_ON: u16 = 0x16;
const VK_IME_OFF: u16 = 0x1A;
// the Zenkaku/Hankaku key sends these by turns
const VK_OEM_AUTO: u16 = 0xF3;
const VK_OEM_ENLW: u16 = 0xF4;

/// Lets timers and posted messages of the text service run, as an
/// application's message loop would between keys.
fn pump(duration: Duration) {
    let deadline = Instant::now() + duration;
    loop {
        unsafe {
            let mut msg = MSG::default();
            while PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE).as_bool() {
                let _ = TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
        }
        if Instant::now() >= deadline {
            return;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
}

extern "system" fn window_proc(window: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    unsafe { DefWindowProcW(window, msg, wparam, lparam) }
}

/// Creates the window the documents belong to and gives it the keyboard
/// focus. TSF hands keys to the text service only while a window of this
/// thread has the focus, and when the focus arrives it selects the input
/// profile of the thread's input language: the window must have it before
/// azooKey is activated.
fn create_focused_window() -> Result<HWND> {
    unsafe {
        let instance = GetModuleHandleW(None)?;
        let class = WNDCLASSW {
            lpfnWndProc: Some(window_proc),
            hInstance: instance.into(),
            lpszClassName: w!("AzookeyTsfTest"),
            ..Default::default()
        };
        RegisterClassW(&class);
        let window = CreateWindowExW(
            WINDOW_EX_STYLE::default(),
            w!("AzookeyTsfTest"),
            w!("azooKey TSF test"),
            WS_OVERLAPPEDWINDOW,
            100,
            100,
            480,
            160,
            None,
            None,
            instance,
            None,
        )?;
        let _ = ShowWindow(window, SW_SHOW);
        ensure!(
            SetForegroundWindow(window).as_bool(),
            "the test window could not become the foreground window"
        );
        SetFocus(window)?;
        pump(Duration::from_millis(200));
        ensure!(GetFocus() == window, "the test window did not get the keyboard focus");
        Ok(window)
    }
}

/// A TSF document backed by a [`TextStore`].
struct Document {
    manager: ITfDocumentMgr,
    context: ITfContext,
    store: ITextStoreACP,
}

impl Document {
    fn create(thread: &ITfThreadMgr, client: u32, window: HWND) -> Result<Self> {
        unsafe {
            let store = TextStore::create(window);
            let manager = thread.CreateDocumentMgr()?;
            let mut context = None;
            let mut cookie = 0;
            manager.CreateContext(client, 0, &store, &mut context, &mut cookie)?;
            let context = context.context("CreateContext returned no context")?;
            manager.Push(&context)?;
            Ok(Self {
                manager,
                context,
                store,
            })
        }
    }

    fn text(&self) -> String {
        store::contents(&self.store).text()
    }

    /// The text after `before`, which the document showed when the
    /// composition started.
    fn after(&self, before: &str) -> String {
        let text = self.text();
        text.strip_prefix(before).unwrap_or(&text).to_string()
    }

    fn compositions(&self) -> Result<usize> {
        unsafe {
            let compositions = self.context.cast::<ITfContextComposition>()?.EnumCompositions()?;
            let mut count = 0;
            loop {
                let mut view = [None];
                let mut fetched = 0;
                compositions.Next(&mut view, &mut fetched)?;
                if fetched == 0 {
                    return Ok(count);
                }
                count += 1;
            }
        }
    }
}

struct Host {
    thread: ITfThreadMgr,
    keystrokes: ITfKeystrokeMgr,
    profiles: ITfInputProcessorProfileMgr,
    /// The keyboard profile that was active before azooKey.
    previous: TF_INPUTPROCESSORPROFILE,
    first: Document,
    second: Document,
}

impl Host {
    /// Sends one key press and release; true when the IME ate the key
    /// press, false when it would have reached the application.
    fn key(&self, key: u16) -> Result<bool> {
        let scan = unsafe { MapVirtualKeyW(key.into(), MAPVK_VK_TO_VSC) } as isize;
        let down = LPARAM(1 | (scan << 16));
        let up = LPARAM(1 | (scan << 16) | (1 << 30) | (1 << 31));
        let eaten = unsafe {
            let eaten = if self.keystrokes.TestKeyDown(WPARAM(key.into()), down)?.as_bool() {
                self.keystrokes.KeyDown(WPARAM(key.into()), down)?.as_bool()
            } else {
                false
            };
            if self.keystrokes.TestKeyUp(WPARAM(key.into()), up)?.as_bool() {
                let _ = self.keystrokes.KeyUp(WPARAM(key.into()), up)?;
            }
            eaten
        };
        pump(Duration::from_millis(30));
        Ok(eaten)
    }

    fn press(&self, key: u16, name: &str) -> Result<()> {
        ensure!(self.key(key)?, "the IME did not handle {name}");
        Ok(())
    }

    fn type_romaji(&self, romaji: &str) -> Result<()> {
        for letter in romaji.chars() {
            let key = letter.to_ascii_uppercase() as u16;
            ensure!(self.key(key)?, "the IME did not handle the key {letter:?} of {romaji:?}");
        }
        Ok(())
    }

    fn focus(&self, document: &Document) -> Result<()> {
        unsafe { self.thread.SetFocus(&document.manager)? };
        pump(Duration::from_millis(50));
        Ok(())
    }

    /// Ends whatever a failed check left behind, so the next one starts
    /// without a composition.
    fn reset(&self) {
        for _ in 0..2 {
            let _ = self.key(VK_ESCAPE);
        }
    }
}

/// Typing starts a composition with the kana; deleting its last character
/// ends the composition and removes the text.
fn last_backspace_ends_the_composition(host: &Host) -> Result<()> {
    let doc = &host.first;
    let before = doc.text();
    host.type_romaji("a")?;
    ensure!(doc.compositions()? == 1, "typing \"a\" started no composition");
    ensure!(doc.after(&before) == "あ", "typing \"a\" shows {:?}", doc.after(&before));
    host.press(VK_BACK, "Backspace")?;
    ensure!(
        doc.compositions()? == 0,
        "deleting the last character left the composition open"
    );
    ensure!(
        doc.text() == before,
        "deleting the last character left {:?}",
        doc.after(&before)
    );
    Ok(())
}

fn escape_cancels(host: &Host) -> Result<()> {
    let doc = &host.first;
    let before = doc.text();
    host.type_romaji("ka")?;
    ensure!(doc.compositions()? == 1, "typing \"ka\" started no composition");
    host.press(VK_ESCAPE, "Escape")?;
    ensure!(doc.compositions()? == 0, "Escape left the composition open");
    ensure!(doc.text() == before, "Escape left {:?}", doc.after(&before));
    Ok(())
}

fn space_converts_and_enter_commits(host: &Host) -> Result<()> {
    let doc = &host.first;
    let before = doc.text();
    host.type_romaji("toukyou")?;
    host.press(VK_SPACE, "Space")?;
    host.press(VK_RETURN, "Enter")?;
    ensure!(doc.compositions()? == 0, "Enter left the composition open");
    ensure!(
        doc.after(&before) == "東京",
        "\"toukyou\" + Space + Enter committed {:?}",
        doc.after(&before)
    );
    Ok(())
}

/// Enter commits exactly what the composition shows.
fn enter_commits_what_is_shown(host: &Host) -> Result<()> {
    let doc = &host.first;
    let before = doc.text();
    host.type_romaji("kyouhaiitenki")?;
    let shown = doc.after(&before);
    ensure!(!shown.is_empty(), "typing showed nothing");
    host.press(VK_RETURN, "Enter")?;
    ensure!(doc.compositions()? == 0, "Enter left the composition open");
    ensure!(
        doc.after(&before) == shown,
        "Enter committed {:?}, but the composition showed {shown:?}",
        doc.after(&before)
    );
    Ok(())
}

/// With live conversion the candidates are listed while typing, with the
/// first selected: one Down selects the second, and Up the first again.
fn down_moves_within_the_live_candidates(host: &Host) -> Result<()> {
    let doc = &host.first;
    let before = doc.text();
    host.type_romaji("kanji")?;
    let first = doc.after(&before);
    host.press(VK_DOWN, "Down")?;
    let second = doc.after(&before);
    ensure!(
        second != first,
        "one Down kept {first:?}; it should select the second candidate"
    );
    host.press(VK_UP, "Up")?;
    let back = doc.after(&before);
    ensure!(back == first, "Up after Down shows {back:?}, not the first candidate {first:?}");
    host.press(VK_ESCAPE, "Escape")?;
    ensure!(doc.text() == before, "Escape left {:?}", doc.after(&before));
    Ok(())
}

/// In Latin mode letters reach the application untouched.
fn latin_mode_passes_letters_through(host: &Host) -> Result<()> {
    let doc = &host.first;
    let before = doc.text();
    host.press(VK_IME_OFF, "VK_IME_OFF")?;
    let eaten = host.key(u16::from(b'A'))?;
    host.press(VK_IME_ON, "VK_IME_ON")?;
    ensure!(!eaten, "the IME ate \"A\" in Latin mode");
    ensure!(doc.compositions()? == 0, "\"A\" in Latin mode started a composition");
    ensure!(doc.text() == before, "\"A\" in Latin mode wrote {:?}", doc.after(&before));
    Ok(())
}

/// Moving to another document mid-composition leaves the text in the
/// first one; the next key composes in the new one only.
fn focus_change_keeps_the_text_in_its_document(host: &Host) -> Result<()> {
    let (first, second) = (&host.first, &host.second);
    let first_before = first.text();
    let second_before = second.text();
    host.type_romaji("ka")?;
    host.focus(second)?;
    host.type_romaji("a")?;
    let result = (|| {
        ensure!(
            first.compositions()? == 0,
            "the first document still has a composition after the focus moved"
        );
        ensure!(
            first.after(&first_before) == "か",
            "the first document ended with {:?} instead of the typed \"か\"",
            first.after(&first_before)
        );
        ensure!(
            second.after(&second_before) == "あ",
            "the second document shows {:?} instead of \"あ\"",
            second.after(&second_before)
        );
        Ok(())
    })();
    host.reset();
    host.focus(first)?;
    result
}

/// F6 to F10 show the input as hiragana, katakana, half-width katakana,
/// full-width and half-width Latin; Enter commits that text.
fn function_keys_show_the_input_as_kana_and_latin(host: &Host) -> Result<()> {
    let doc = &host.first;
    for (key, name, expected) in [
        (VK_F6, "F6", "とうきょう"),
        (VK_F7, "F7", "トウキョウ"),
        (VK_F8, "F8", "ﾄｳｷｮｳ"),
        (VK_F9, "F9", "ｔｏｕｋｙｏｕ"),
        (VK_F10, "F10", "toukyou"),
    ] {
        let before = doc.text();
        host.type_romaji("toukyou")?;
        host.press(key, name)?;
        let shown = doc.after(&before);
        ensure!(shown == expected, "{name} shows {shown:?}, not {expected:?}");
        host.press(VK_RETURN, "Enter")?;
        ensure!(doc.compositions()? == 0, "Enter after {name} left the composition open");
        ensure!(
            doc.after(&before) == expected,
            "Enter after {name} committed {:?}, not {expected:?}",
            doc.after(&before)
        );
    }
    Ok(())
}

/// Typing while a conversion is shown commits the conversion and composes
/// the new text after it.
fn typing_after_a_conversion_commits_it(host: &Host) -> Result<()> {
    let doc = &host.first;
    let before = doc.text();
    host.type_romaji("toukyou")?;
    host.press(VK_SPACE, "Space")?;
    let converted = doc.after(&before);
    ensure!(!converted.is_empty(), "Space showed nothing");
    host.type_romaji("a")?;
    ensure!(doc.compositions()? == 1, "typing after the conversion left no composition");
    ensure!(
        doc.after(&before) == format!("{converted}あ"),
        "typing \"a\" after {converted:?} shows {:?}",
        doc.after(&before)
    );
    host.press(VK_ESCAPE, "Escape")?;
    ensure!(doc.compositions()? == 0, "Escape left the composition open");
    ensure!(
        doc.after(&before) == converted,
        "Escape left {:?}; only \"あ\" after the committed {converted:?} should go",
        doc.after(&before)
    );
    Ok(())
}

/// In Kana mode a digit starts a composition like a letter.
fn digits_start_a_composition(host: &Host) -> Result<()> {
    let doc = &host.first;
    let before = doc.text();
    host.press(u16::from(b'1'), "1")?;
    ensure!(doc.compositions()? == 1, "typing \"1\" started no composition");
    let shown = doc.after(&before);
    ensure!(!shown.is_empty(), "typing \"1\" showed nothing");
    host.press(VK_RETURN, "Enter")?;
    ensure!(doc.compositions()? == 0, "Enter left the composition open");
    ensure!(
        doc.after(&before) == shown,
        "Enter committed {:?}, but the composition showed {shown:?}",
        doc.after(&before)
    );
    Ok(())
}

/// Zenkaku/Hankaku during a composition commits it and switches to Latin
/// mode; pressed again it switches back to Kana mode.
fn zenkaku_hankaku_commits_and_toggles_the_mode(host: &Host) -> Result<()> {
    let doc = &host.first;
    let before = doc.text();
    host.type_romaji("ka")?;
    let result = (|| {
        host.press(VK_OEM_AUTO, "Zenkaku/Hankaku")?;
        ensure!(doc.compositions()? == 0, "Zenkaku/Hankaku left the composition open");
        ensure!(
            doc.after(&before) == "か",
            "Zenkaku/Hankaku committed {:?} instead of \"か\"",
            doc.after(&before)
        );
        ensure!(
            !host.key(u16::from(b'A'))?,
            "the IME ate \"A\" after Zenkaku/Hankaku switched to Latin mode"
        );
        host.press(VK_OEM_ENLW, "Zenkaku/Hankaku")?;
        let kana = doc.text();
        host.type_romaji("a")?;
        ensure!(
            doc.after(&kana) == "あ",
            "after Zenkaku/Hankaku switched back, \"a\" shows {:?}",
            doc.after(&kana)
        );
        host.press(VK_ESCAPE, "Escape")
    })();
    // the next check starts in Kana mode whatever happened here
    let _ = host.key(VK_IME_ON);
    result
}

/// Switching to another input method in the middle of a composition ends
/// it and keeps its text.
fn switching_the_input_method_keeps_the_composition_text(host: &Host) -> Result<()> {
    let doc = &host.first;
    let before = doc.text();
    host.type_romaji("ka")?;
    let previous = &host.previous;
    unsafe {
        host.profiles
            .ActivateProfile(
                previous.dwProfileType,
                previous.langid,
                &previous.clsid,
                &previous.guidProfile,
                previous.hkl,
                TF_IPPMF_FORPROCESS | TF_IPPMF_DONTCARECURRENTINPUTLANGUAGE,
            )
            .context("switching back to the previous input method")?;
    }
    pump(Duration::from_millis(200));
    ensure!(
        unsafe { host.keystrokes.GetForeground()? } != CLSID_AZOOKEY,
        "azooKey is still the foreground text service after the switch"
    );
    ensure!(
        doc.compositions()? == 0,
        "switching the input method left the composition open"
    );
    ensure!(
        doc.after(&before) == "か",
        "switching the input method during a composition left {:?}",
        doc.after(&before)
    );
    Ok(())
}

/// The full path of the loaded module named like `expected`.
fn loaded_module(expected: &Path) -> Result<PathBuf> {
    let name = expected.file_name().context("--expect-dll needs a file name")?;
    unsafe {
        let module = GetModuleHandleW(&HSTRING::from(name))
            .with_context(|| format!("{} is not loaded", name.to_string_lossy()))?;
        let mut path = [0u16; 1024];
        let len = GetModuleFileNameW(module, &mut path) as usize;
        Ok(PathBuf::from(String::from_utf16_lossy(&path[..len])))
    }
}

fn same_file(a: &Path, b: &Path) -> bool {
    match (std::fs::canonicalize(a), std::fs::canonicalize(b)) {
        (Ok(a), Ok(b)) => a == b,
        _ => false,
    }
}

fn parse_args() -> Result<Option<PathBuf>> {
    let mut expected = None;
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--expect-dll" => expected = Some(args.next().context("--expect-dll needs a path")?.into()),
            other => bail!("unknown argument: {other}"),
        }
    }
    Ok(expected)
}

/// The Japanese profiles TSF lists in this process, to explain a failed setup.
fn japanese_profiles(profiles: &ITfInputProcessorProfileMgr) -> String {
    let mut lines = Vec::new();
    unsafe {
        match profiles.EnumProfiles(LANGID_JAPANESE) {
            Ok(found) => loop {
                let mut profile = [TF_INPUTPROCESSORPROFILE::default()];
                let mut fetched = 0;
                if found.Next(&mut profile, &mut fetched).is_err() || fetched == 0 {
                    break;
                }
                let [profile] = profile;
                lines.push(format!(
                    "type {} {:?} {:?} flags {:#x}",
                    profile.dwProfileType, profile.clsid, profile.guidProfile, profile.dwFlags
                ));
            },
            Err(error) => lines.push(error.to_string()),
        }
    }
    format!("Japanese profiles TSF lists:\n    {}", lines.join("\n    "))
}

/// TSF lists a profile that another process (regsvr32, the installer) has
/// just registered only after several seconds; until then, activating it
/// fails with E_FAIL. Returns how long it took.
fn wait_for_profile(profiles: &ITfInputProcessorProfileMgr) -> Result<Duration> {
    const TIMEOUT: Duration = Duration::from_secs(60);
    let started = Instant::now();
    loop {
        let mut profile = TF_INPUTPROCESSORPROFILE::default();
        let listed = unsafe {
            profiles.GetProfile(
                TF_PROFILETYPE_INPUTPROCESSOR,
                LANGID_JAPANESE,
                &CLSID_AZOOKEY,
                &PROFILE_AZOOKEY,
                HKL::default(),
                &mut profile,
            )
        };
        match listed {
            Ok(()) => return Ok(started.elapsed()),
            Err(error) if started.elapsed() >= TIMEOUT => bail!(
                "TSF does not list the azooKey profile after {TIMEOUT:?} (is the DLL registered?): {error}\n  {}",
                japanese_profiles(profiles)
            ),
            Err(_) => pump(Duration::from_millis(250)),
        }
    }
}

fn start(expected: Option<&Path>) -> Result<Host> {
    unsafe {
        CoInitializeEx(None, COINIT_APARTMENTTHREADED).ok()?;
        let window = create_focused_window()?;

        let thread: ITfThreadMgr = CoCreateInstance(&CLSID_TF_ThreadMgr, None, CLSCTX_INPROC_SERVER)?;
        let client = thread.Activate()?;
        let first = Document::create(&thread, client, window)?;
        let second = Document::create(&thread, client, window)?;
        thread.SetFocus(&first.manager)?;

        let profiles: ITfInputProcessorProfileMgr =
            CoCreateInstance(&CLSID_TF_InputProcessorProfiles, None, CLSCTX_INPROC_SERVER)?;
        let mut previous = TF_INPUTPROCESSORPROFILE::default();
        profiles.GetActiveProfile(&GUID_TFCAT_TIP_KEYBOARD, &mut previous)?;
        ensure!(
            previous.clsid != CLSID_AZOOKEY,
            "azooKey is active already; start with another input method so the test can switch back to it"
        );
        let waited = wait_for_profile(&profiles)?;
        println!("TSF listed the azooKey profile after {:.1} s", waited.as_secs_f64());
        profiles
            .ActivateProfile(
                TF_PROFILETYPE_INPUTPROCESSOR,
                LANGID_JAPANESE,
                &CLSID_AZOOKEY,
                &PROFILE_AZOOKEY,
                HKL::default(),
                TF_IPPMF_FORPROCESS | TF_IPPMF_DONTCARECURRENTINPUTLANGUAGE,
            )
            .context("activating the azooKey profile")?;
        pump(Duration::from_millis(200));

        let mut active = TF_INPUTPROCESSORPROFILE::default();
        profiles.GetActiveProfile(&GUID_TFCAT_TIP_KEYBOARD, &mut active)?;
        ensure!(
            active.clsid == CLSID_AZOOKEY,
            "the active keyboard text service is {:?}, not azooKey",
            active.clsid
        );
        let keystrokes: ITfKeystrokeMgr = thread.cast()?;
        ensure!(
            keystrokes.GetForeground()? == CLSID_AZOOKEY,
            "azooKey is not the foreground text service"
        );

        if let Some(expected) = expected {
            let loaded = loaded_module(expected)?;
            ensure!(
                same_file(&loaded, expected),
                "the text service was loaded from {}, not {}",
                loaded.display(),
                expected.display()
            );
            println!("text service loaded from {}", loaded.display());
        }

        Ok(Host {
            thread,
            keystrokes,
            profiles,
            previous,
            first,
            second,
        })
    }
}

fn main() -> ExitCode {
    let expected = match parse_args() {
        Ok(expected) => expected,
        Err(error) => {
            eprintln!("{error:#}");
            return ExitCode::from(2);
        }
    };
    let host = match start(expected.as_deref()) {
        Ok(host) => host,
        Err(error) => {
            eprintln!("FAILED setup: {error:#}");
            return ExitCode::FAILURE;
        }
    };

    let scenarios: [(&str, fn(&Host) -> Result<()>); 12] = [
        ("Kana mode", |host| host.press(VK_IME_ON, "VK_IME_ON")),
        ("last Backspace ends the composition", last_backspace_ends_the_composition),
        ("Escape cancels", escape_cancels),
        ("Space converts and Enter commits", space_converts_and_enter_commits),
        ("Enter commits what is shown", enter_commits_what_is_shown),
        ("Down moves within the live candidates", down_moves_within_the_live_candidates),
        ("F6 to F10 show the input as kana and Latin", function_keys_show_the_input_as_kana_and_latin),
        ("typing after a conversion commits it", typing_after_a_conversion_commits_it),
        ("digits start a composition", digits_start_a_composition),
        ("Zenkaku/Hankaku commits and toggles the mode", zenkaku_hankaku_commits_and_toggles_the_mode),
        ("Latin mode passes letters through", latin_mode_passes_letters_through),
        ("focus change keeps the text in its document", focus_change_keeps_the_text_in_its_document),
    ];
    let mut failures = 0;
    for (name, scenario) in scenarios {
        match scenario(&host) {
            Ok(()) => println!("ok     {name}"),
            Err(error) => {
                failures += 1;
                println!("FAILED {name}: {error:#}");
                host.reset();
            }
        }
    }
    // last: azooKey is no longer active afterwards
    match switching_the_input_method_keeps_the_composition_text(&host) {
        Ok(()) => println!("ok     switching the input method keeps the composition text"),
        Err(error) => {
            failures += 1;
            println!("FAILED switching the input method keeps the composition text: {error:#}");
        }
    }

    if failures == 0 {
        println!("TSF test passed");
        ExitCode::SUCCESS
    } else {
        println!("{failures} TSF check(s) failed");
        ExitCode::FAILURE
    }
}
