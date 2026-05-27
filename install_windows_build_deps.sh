#!/bin/bash
# Windows 크로스 컴파일에 필요한 시스템 패키지 설치

set -e

if [ "$(id -u)" -ne 0 ]; then
    echo "root 권한이 필요합니다. sudo로 실행해주세요."
    exit 1
fi

# LLVM 버전 탐지: clang-XX 바이너리에서 버전 추출
LLVM_VERSION=""
for f in /usr/bin/clang-[0-9]*; do
    [ -e "$f" ] && LLVM_VERSION=$(basename "$f" | sed 's/clang-//') && break
done
# clang이 아직 없으면 설치 후 탐지
if [ -z "$LLVM_VERSION" ]; then
    apt update
    apt install -y clang
    for f in /usr/bin/clang-[0-9]*; do
        [ -e "$f" ] && LLVM_VERSION=$(basename "$f" | sed 's/clang-//') && break
    done
fi
if [ -z "$LLVM_VERSION" ]; then
    echo "LLVM 버전을 탐지할 수 없습니다."
    exit 1
fi

echo "탐지된 LLVM 버전: ${LLVM_VERSION}"

apt update
apt install -y clang lld llvm
if ! apt install -y "clang-tools-${LLVM_VERSION}"; then
    echo "경고: clang-tools-${LLVM_VERSION} 을 찾을 수 없음, clang-tools 시도..."
    apt install -y clang-tools || echo "경고: clang-tools 설치 실패 (clang-cl 미설치 가능성)"
fi

# 버전 없는 심볼릭 링크 생성 (cargo-xwin이 필요로 함)
for tool in llvm-lib llvm-dlltool llvm-rc clang-cl; do
    if [ -f "/usr/bin/${tool}-${LLVM_VERSION}" ] && [ ! -e "/usr/bin/${tool}" ] && [ ! -L "/usr/bin/${tool}" ]; then
        ln -s "${tool}-${LLVM_VERSION}" "/usr/bin/${tool}"
        echo "심볼릭 링크 생성: ${tool} → ${tool}-${LLVM_VERSION}"
    elif [ -e "/usr/bin/${tool}" ] || [ -L "/usr/bin/${tool}" ]; then
        echo "이미 존재: /usr/bin/${tool}"
    else
        echo "경고: /usr/bin/${tool}-${LLVM_VERSION} 을 찾을 수 없습니다"
    fi
done

echo ""
echo "설치 완료:"
clang --version | head -1
ld.lld --version | head -1
llvm-lib --version 2>/dev/null | head -1 || echo "llvm-lib: 미설치"
clang-cl --version 2>/dev/null | head -1 || echo "clang-cl: 미설치"
