# azooKey for Windows

[AzooKeyKanaKanjiConverter](https://github.com/azooKey/AzooKeyKanaKanjiConverter)を利用したWindows版IMEです。

> [!WARNING]
> 現在開発中であるため、安定性や機能に関しては保証できません。使用する際は自己責任でお願いします。

# インストール方法
[Release](https://github.com/fkunn1326/azooKey-Windows/releases)から`azookey-setup.exe`をダウンロードし、インストーラーを実行してください。

インストールには管理者権限が必要です。インストール先は`C:\Program Files\Azookey`に固定されており、`/DIR=`などで別のフォルダーを指定することはできません。変換エンジンはログオン時に、そのPCのすべてのユーザーで起動します。

# 機能

- [x] ライブ変換
- [x] Zenzaiを使用したニューラルかな漢字変換

- [x] 学習機能
- [x] 辞書登録機能
- [x] テーマ変更機能
- [x] 辞書のインポート/エクスポート機能
- [x] いい感じ変換
- [x] 個人最適化システム
- [x] 予測変換

# 設定

設定は`%APPDATA%\Azookey\settings.json`に保存されます。不正な値がある項目は既定値として読み込み、ほかの項目はそのまま使います。ファイル自体を読み込めない場合、設定画面はエラーを表示し、ファイルを上書きしません。

## 入力モード切替ショートカット

- `Ctrl + Space`: 英数/かなを切り替え
- `` Alt + ` ``: 英数/かなを切り替え
- `VK_IME_ON` (`0x16`): かな入力へ切り替え
- `VK_IME_OFF` (`0x1A`): 英数入力へ切り替え

`Ctrl + Space` と `` Alt + ` `` は設定画面から個別に無効化できます。Shift + Spaceは一般的なアプリ操作と競合しやすいため割り当てていません。
## 学習

一般設定から学習モードを選択できます。学習履歴はリセット可能です。

## ユーザー辞書

「読み → 単語」を登録できます。JSONとTSVに加えて、Microsoft IME系のUTF-16テキスト辞書のインポート・エクスポートに対応しています。

## 外観

候補ウィンドウの配色、背景画像、カスタムCSSを設定できます。

## 予測変換

有効にすると、変換確定後にKanaKanjiConverterの確定後予測候補を表示します。候補確定後も続けて予測できます。

## いい感じ変換

OpenAI互換Chat Completions APIを設定すると、入力中にF5を押した時だけAI変換候補を生成します。外部APIのエンドポイントはHTTPS必須で、HTTPはループバックアドレス（`localhost`、`127.0.0.1`、`[::1]`など）のみ許可しています。APIキーを設定した場合はローカルの設定ファイルに保存されるため、設定ファイルを共有しないでください。

## 個人最適化

Zenzai有効時に、Tunerが生成した `p13n_v1` ディレクトリを指定できます。`original_*.marisa` と `lm_*.marisa` の必要ファイルを検証してから個人N-gramモデルを読み込みます。

## Zenzai

### 変換プロファイル
設定で変換プロファイルを指定すると、プロファイルに応じた変換候補が表示されます。

### 個人最適化
[Tuner](https://github.com/azooKey/Tuner)で生成した `p13n_v1` ディレクトリを設定画面から指定できます。
`original_*.marisa` と `lm_*.marisa` の必須モデルを確認してからZenzaiへ読み込みます。

### バックエンド
以下の3種類のバックエンドをサポートしています。新規設定ではVulkanを既定にし、Vulkanランタイムを利用できない場合はlauncherがCPUへフォールバックします。

- **CPU**: 動作が非常に遅いため、非推奨です。
- **CUDA**: NvidiaのGPU専用。[CUDA Toolkit 12系](https://developer.nvidia.com/cuda-downloads)をインストールする必要があります。
- **Vulkan**: GPUのドライバーに標準で含まれているため、追加のインストールは不要です。

## ユーザー辞書

設定画面から「読み → 単語」を登録できます。登録内容は設定ファイルへ保存され、変換サーバーへ即時反映されます。読みと単語には、タブや改行などの制御文字を使えません。

JSON、TSV、Microsoft IME形式のテキスト辞書をインポートできます。エクスポートはJSON、TSV、Microsoft IME向けUTF-16LEテキストに対応しています。

## テーマ

候補ウィンドウのライト/ダーク/システム配色、背景色、文字色、アクセント色、選択色、背景画像、カスタムCSSを設定できます。4 MiBを超える背景画像は表示されません。

## いい感じ変換

設定画面でOpenAI互換APIのエンドポイント、モデル、APIキー、変換指示を設定できます。通常入力中には外部APIへ送信しません。変換対象がある状態で `F5` を押した時だけAPIへ送り、最大5件の変換候補を表示します。

## ライブ変換

設定画面からライブ変換を切り替えられます。有効時は入力中に変換候補を更新します。無効時は通常の打鍵では変換候補を計算せず、SpaceまたはTabを押した時に変換を開始します。

## 予測変換

有効化すると通常候補を確定した直後にKanaKanjiConverterの確定後予測候補を表示します。Enterで候補を確定すると、続く予測候補も表示できます。

# コミュニティ

## 開発を支援する
- [GitHub Sponsors (Miwa)](https://github.com/sponsors/ensan-hcl): 変換エンジンの開発者
- [Patreon (fkunn1326)](https://www.patreon.com/c/fkunn1326): Windowsに移植した人

## 開発に参加する

### 開発環境のセットアップ

- [Rust](https://www.rust-lang.org/tools/install) (1.89以上)
- [Swift for Windows](https://www.swift.org/install/windows/) (Swift 6.3.3。CIと同じバージョンです。Swift 6.0系ではC++相互運用を含むビルドが現在のMSVC標準ライブラリに対応していません)
- [protoc](https://protobuf.dev/installation/) 
- [node.js](https://nodejs.org/en/download/)
- [inno setup](https://jrsoftware.org/isinfo.php) (6.4以上)

### ビルド

#### リポジトリのクローン
```
git clone https://github.com/fkunn1326/azooKey-Windows --recursive
```
`--recursive`オプションを付けて、サブモジュールも一緒にクローンしてください。

#### cargo-makeのインストール
```
cargo install --force cargo-make
```

#### ビルド
```
cargo make build [--debug/--release]
```
`--debug`オプションを付けるとデバッグビルド、`--release`オプションを付けるとリリースビルドになります。必ずどちらかを指定してください。

`build`フォルダーが作成され、ビルドされた実行ファイルが格納されます。

`launcher.exe`を管理者権限で実行すると、azookeyの変換エンジンと候補ウィンドウが起動します。launcherはこれらが終了すると自動で再起動するため、止める場合は先に`launcher.exe`を終了してください。launcherは1つのセッションにつき1つだけ動作します。管理者権限なしで実行した場合、候補ウィンドウはUIAccessなしで動作するため、管理者権限で動くアプリなどの上には表示されません。

また、IMEを登録する際は以下のように`regsvr32.exe`を使用して登録する必要があります。
```c
regsvr32.exe "path/to/build/azookey_windows.dll" /s
regsvr32.exe "path/to/build/x86/azookey_windows.dll" /s
```
逆に登録を解除する場合は`/u`オプションを付けて実行してください。

#### 開発時のヒント
- 開発は仮想マシンまたは専用のPCで行うことを推奨します。IMEがクラッシュするとWindowsがフリーズする可能性があります。
- IMEを解除する際、IMEを使用中のアプリケーション（メモ帳など）を終了しないと、解除できないことがあります。
- デバッグビルドのIMEは`%LOCALAPPDATA%\Azookey\logs`にトレースログを出力します。Chromeのトレースビューアーで開けます。

#### テスト
CIでは、ビルドしたIMEのDLLを`azookey-tsf-test`がアプリと同じようにTSF経由で読み込み、キー入力に対して文書に表示される内容を確認します。IMEがクラッシュした場合もテストは失敗します。

手元で実行する場合は、DLLを登録して`launcher.exe`を起動してから、DLLと同じビット数のテストを実行してください。テスト用のウィンドウがキーボードフォーカスを取るため、実行中はPCを操作しないでください。
```
cargo build --release -p tsf-test
target\release\azookey-tsf-test.exe --expect-dll "path/to/build/azookey_windows.dll"

cargo build --release -p tsf-test --target i686-pc-windows-msvc
target\i686-pc-windows-msvc\release\azookey-tsf-test.exe --expect-dll "path/to/build/x86/azookey_windows.dll"
```

# 関連

- [azooKey/azooKey](https://github.com/azooKey/azooKey): iOS / iPadOS向けの日本語キーボードアプリ
- [7ka-Hiira/fcitx5-hazkey](https://github.com/7ka-Hiira/fcitx5-hazkey): fcitx5向けのLinux版azooKey
- [azooKey/AzookeyKanakanjiConverter](https://github.com/azooKey/AzooKeyKanaKanjiConverter): azooKeyの変換エンジン

# 参考
本プロジェクトの開発にあたり、以下のリソースを参考にしました。ありがとうございます！
- [OMAMA-Taioan/khiin-rs](https://github.com/OMAMA-Taioan/khiin-rs/tree/master/windows)
- [google/mozc](https://github.com/google/mozc/tree/master/src/win32/tip)
- [microsoft/Windows-classic-samples](https://github.com/microsoft/Windows-classic-samples/tree/main/Samples/Win7Samples/winui/input/tsf/textservice)
- [dec32/ajemi](https://github.com/dec32/ajemi)
- https://zenn.dev/mkpoli/scraps/6dc57fcd0335cf
