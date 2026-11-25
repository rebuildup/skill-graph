# Security Policy

skill-graph は信頼できないスキル文書を解析するツールである。スキル内の script、外部リンク、パス、ログを実行・取得・永続化しないことを前提に設計する。

## Scope

- `SKILL.md` と同梱ファイルの解析
- Markdown / YAML / URI / path 解決
- CLI、Hook、Plugin、評価ハーネス

## Reporting

脆弱性や安全性の問題を見つけた場合は、公開 Issue に詳細を書かず、リポジトリ公開後は GitHub Security Advisories またはメンテナへ直接連絡する。リポジトリ公開前の連絡先は未確定である。

## Security invariants

- スキル配下の script を解析目的で実行しない。
- 探索 root 外への path traversal、symlink 越境を拒否する。
- 外部 URL を自動取得しない。
- ユーザープロンプト、会話全文、秘密情報を永続保存・外部送信しない。
- エージェントの権限・承認・sandbox・Hook 信頼境界を迂回しない。
