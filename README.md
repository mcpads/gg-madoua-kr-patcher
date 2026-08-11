# 마도물어 A (게임기어) 한글 패처

게임기어용 《마도물어 A》 일본판에 한글 패치를 적용하는 Rust 코드입니다. 일본판 원본 검증, 텍스트 추출·재배치, 한글 폰트와 UI 생성, checked Z80 훅, BPS 생성·왕복 검증 기능을 제공합니다.

영어판 ROM·영어 패치·영어판 자산은 포함하지 않으며 한글 빌드에도 필요하지 않습니다.

배포용 BPS와 적용 방법은 [마도물어 시리즈 한글 번역 프로젝트](https://github.com/mcpads/madou-monogatari-kr-patch#마도물어-a--게임기어-베타)에서 제공합니다. 이 저장소는 현재 패처 코드 스냅샷이므로, 별도 배포 저장소의 과거 BPS와 항상 바이트 단위로 같다고 주장하지 않습니다.

## 제공하지 않는 파일

- 원본·패치 적용 ROM과 BPS 산출물
- 대사 JSON과 검토 상태를 비롯한 번역 자산
- 한글 폰트, UI 그래픽과 생성 바이너리
- 영어판 ROM·패치와 영어 대조 자료
- 내부 문서, QA manifest·화면, 작업 기록과 런타임 증거

정당한 원본과 프로젝트 입력을 가진 사용자가 번역 디렉터리와 폰트를 직접 준비해야 합니다. 현재 작업 경로의 예시는 다음과 같습니다.

```text
assets/fonts/dalmoori.ttf
assets/translations/scripts/complete/
```

사람 검수 후보를 재현할 때는 해당 자산을 가진 사용자가 `needs_human_review` 디렉터리와 `--preview-human-review`를 명시해야 합니다. 이 옵션은 해당 후보를 배포 적합 상태로 승격하지 않습니다.

지원 일본판 ROM은 524,288바이트이며 SHA-256은 다음과 같습니다.

```text
6679b88d3db2ca62a78b1904cfe8364f7e6d5d74ffda27b7dbe49417ed2d02ec
```

## 빌드와 검사

```bash
cargo build --release --locked --workspace
cargo test --locked --workspace

cargo run --release --locked -- build \
  --rom "<일본판 ROM>.gg" \
  --translations assets/translations \
  --font assets/fonts/dalmoori.ttf \
  --output out/gg-madoua-kr.gg \
  --bps-output out/gg-madoua-kr.bps
```

## 라이선스

이 저장소의 소스 코드는 [MIT License](LICENSE)로 제공합니다. 원작 게임과 사용자가 별도로 준비하는 원본·폰트·번역 입력의 권리는 각 권리자에게 있습니다.
