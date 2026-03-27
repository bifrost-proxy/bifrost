#!/usr/bin/env python3
"""
规则文件检查脚本

检查 Whistle 规则文件中操作符后的值是否包含空格。
如果值包含空格，必须使用引用值 {name} 方式。

支持的特殊语法:
- ``` 代码块 - 块内内容跳过检查
- line`...` 多行块 - 块内内容作为单条规则，不检查空格
- includeFilter:// / excludeFilter:// - 过滤器语法
- lineProps:// - 行属性语法

用法:
    python3 check_rules.py                     # 检查所有规则文件
    python3 check_rules.py rules/xxx.txt       # 检查单个文件
    python3 check_rules.py --errors-only       # 仅显示错误
"""

import os
import re
import sys
import argparse
from pathlib import Path
from typing import List, Tuple, Optional


def configure_console_output():
    """
    避免在 Windows 非 UTF-8 终端中因为中文输出导致脚本崩溃。

    保留原有编码，仅把错误策略改为 backslashreplace，这样即使控制台
    无法编码中文，也会输出 \\uXXXX 转义序列而不是抛出 UnicodeEncodeError。
    """
    for stream_name in ("stdout", "stderr"):
        stream = getattr(sys, stream_name, None)
        if stream is None or not hasattr(stream, "reconfigure"):
            continue
        try:
            stream.reconfigure(errors="backslashreplace")
        except Exception:
            continue


class RuleError:
    def __init__(self, file_path: str, line_num: int, line_content: str,
                 operator: str, value: str, message: str):
        self.file_path = file_path
        self.line_num = line_num
        self.line_content = line_content
        self.operator = operator
        self.value = value
        self.message = message

    def suggest_fix(self) -> str:
        safe_name = re.sub(r'[^a-zA-Z0-9_]', '_', self.operator.lower())
        return f"定义引用值 {{{safe_name}_value}} 并使用 {self.operator}://{{{safe_name}_value}}"


SIMPLE_OPERATOR_PATTERN = re.compile(r'([a-zA-Z][a-zA-Z0-9]*):\/\/(\S+)')
OPERATOR_WITH_TRAILING_SPACE_PATTERN = re.compile(r'([a-zA-Z][a-zA-Z0-9]*):\/\/(\s+)')
PAREN_CONTENT_PATTERN = re.compile(r'([a-zA-Z][a-zA-Z0-9]*):\/\/(\([^)]*\))')

LINE_BLOCK_START_PATTERN = re.compile(r'^line`')
LINE_BLOCK_END_PATTERN = re.compile(r'^`$')

CONTROL_OPERATORS = {'includeFilter', 'excludeFilter', 'lineProps'}


def check_value_has_space(value: str) -> Tuple[bool, Optional[str]]:
    """
    检查操作符值是否合法（不含空格）

    返回: (is_valid, error_message)

    特殊情况:
    - 括号内容 (xxx) 中的空格是合法的
    """
    if not value:
        return True, None

    if value.startswith('(') and value.endswith(')'):
        return True, None

    if ' ' in value:
        return False, "值包含空格，空格会被解析为规则分隔符"

    return True, None


def check_incomplete_value(value: str) -> Tuple[bool, Optional[str]]:
    """
    检查值是否因为空格而被截断（不完整）

    例如: ua://Mozilla/5.0 会被正则匹配为 Mozilla/5.0，但用户实际想输入的可能是
          ua://Mozilla/5.0 (iPhone; CPU iPhone OS 15_0 like Mac OS X)

    检测规则:
    1. 以 ( 开头但没有对应的 ) 结尾 - 括号不匹配
    2. 以 { 开头但没有对应的 } 结尾 - 大括号不匹配
    3. 以 ` 开头但没有对应的 ` 结尾 - 反引号不匹配

    返回: (is_valid, error_message)
    """
    if not value:
        return True, None

    if value.startswith('(') and not value.endswith(')'):
        return False, "内联值 () 括号不匹配，可能因空格被截断，请使用引用值 {name}"

    if value.startswith('{') and not value.endswith('}'):
        return False, "引用值 {} 括号不匹配，可能因空格被截断"

    if value.startswith('`') and not value.endswith('`'):
        return False, "模板字符串反引号不匹配，可能因空格被截断"

    if value.count('(') > value.count(')'):
        return False, "值中包含未闭合的括号 (，可能因空格被截断，请使用引用值 {name}"

    return True, None


def check_operator_trailing_space(line: str) -> List[Tuple[str, str]]:
    """
    检查操作符后是否紧跟空格 (xxx:// 后面有空格)

    返回: [(operator, error_marker), ...] 其中 error_marker 为空字符串表示 :// 后有空格

    注意: 如果空格后跟另一个操作符 (如 tlsIntercept:// reqHeaders://...)，
          则不认为是错误（多操作符语法）
    """
    results = []
    for m in OPERATOR_WITH_TRAILING_SPACE_PATTERN.finditer(line):
        operator = m.group(1)
        rest_of_line = line[m.end():]
        if rest_of_line and SIMPLE_OPERATOR_PATTERN.match(rest_of_line):
            continue
        results.append((operator, ''))
    return results


def parse_rule_line(line: str) -> List[Tuple[str, str]]:
    """
    解析规则行，提取所有 operator://value 对

    返回: [(operator, value), ...]

    特殊处理:
    1. 括号内容 protocol://(content with spaces) - 括号内的空格是合法的
    """
    results = []
    processed_ranges = []

    for m in PAREN_CONTENT_PATTERN.finditer(line):
        results.append((m.group(1), m.group(2)))
        processed_ranges.append((m.start(), m.end()))

    for m in SIMPLE_OPERATOR_PATTERN.finditer(line):
        start, end = m.start(), m.end()
        is_overlapping = any(
            (start >= pr_start and start < pr_end) or (end > pr_start and end <= pr_end)
            for pr_start, pr_end in processed_ranges
        )
        if not is_overlapping:
            results.append((m.group(1), m.group(2)))

    return results


def check_file(file_path: str) -> List[RuleError]:
    """
    检查单个规则文件

    返回: 错误列表
    """
    errors = []
    in_code_block = False
    in_line_block = False
    line_block_start = 0
    line_block_content = []

    try:
        with open(file_path, 'r', encoding='utf-8') as f:
            lines = f.readlines()
    except Exception as e:
        print(f"无法读取文件 {file_path}: {e}", file=sys.stderr)
        return errors

    for line_num, line in enumerate(lines, 1):
        stripped = line.strip()

        if stripped.startswith('```'):
            in_code_block = not in_code_block
            continue

        if in_code_block:
            continue

        if in_line_block:
            if LINE_BLOCK_END_PATTERN.match(stripped):
                in_line_block = False
                combined_line = ' '.join(line_block_content)
                block_errors = check_line_block_content(
                    file_path, line_block_start, combined_line
                )
                errors.extend(block_errors)
                line_block_content = []
            else:
                line_block_content.append(stripped)
            continue

        if LINE_BLOCK_START_PATTERN.match(stripped):
            in_line_block = True
            line_block_start = line_num
            after_marker = stripped[5:]
            if after_marker:
                line_block_content.append(after_marker)
            continue

        if not stripped or stripped.startswith('#'):
            continue

        trailing_space_errors = check_operator_trailing_space(stripped)
        for operator, _ in trailing_space_errors:
            if operator not in CONTROL_OPERATORS:
                errors.append(RuleError(
                    file_path=file_path,
                    line_num=line_num,
                    line_content=stripped,
                    operator=operator,
                    value='<空格>',
                    message="操作符 :// 后面不能有空格"
                ))

        operators = parse_rule_line(stripped)

        for operator, value in operators:
            if operator in CONTROL_OPERATORS:
                continue

            is_valid, error_msg = check_value_has_space(value)
            if not is_valid:
                errors.append(RuleError(
                    file_path=file_path,
                    line_num=line_num,
                    line_content=stripped,
                    operator=operator,
                    value=value,
                    message=error_msg
                ))
                continue

            is_valid, error_msg = check_incomplete_value(value)
            if not is_valid:
                errors.append(RuleError(
                    file_path=file_path,
                    line_num=line_num,
                    line_content=stripped,
                    operator=operator,
                    value=value,
                    message=error_msg
                ))

    if in_line_block:
        errors.append(RuleError(
            file_path=file_path,
            line_num=line_block_start,
            line_content='line`...',
            operator='line',
            value='<未闭合>',
            message="line` 块未闭合，缺少结束的 `"
        ))

    return errors


def check_line_block_content(file_path: str, line_num: int, content: str) -> List[RuleError]:
    """
    检查 line` 块内容

    line` 块内的内容会被合并为单行处理，空格是合法的分隔符
    只检查操作符值的完整性（括号匹配等）
    """
    errors = []
    operators = parse_rule_line(content)

    for operator, value in operators:
        if operator in CONTROL_OPERATORS:
            continue

        is_valid, error_msg = check_incomplete_value(value)
        if not is_valid:
            errors.append(RuleError(
                file_path=file_path,
                line_num=line_num,
                line_content=f"line`...` 块",
                operator=operator,
                value=value,
                message=error_msg
            ))

    return errors


def find_rule_files(base_path: str) -> List[str]:
    """
    查找所有规则文件
    """
    rules_dir = os.path.join(base_path, 'rules')
    if not os.path.isdir(rules_dir):
        return []

    files = []
    for root, _, filenames in os.walk(rules_dir):
        for filename in filenames:
            if filename.endswith('.txt'):
                files.append(os.path.join(root, filename))

    return sorted(files)


def print_error(error: RuleError, base_path: str):
    """打印错误信息"""
    rel_path = os.path.relpath(error.file_path, base_path)
    print(f"\n✗ {rel_path}:{error.line_num}")
    print(f"   错误: {error.message}")
    print(f"   当前: {error.operator}://{error.value}")
    print(f"   建议: {error.suggest_fix()}")


def main():
    configure_console_output()

    parser = argparse.ArgumentParser(
        description='检查规则文件中操作符后的值是否包含空格'
    )
    parser.add_argument(
        'files',
        nargs='*',
        help='要检查的文件（默认检查所有规则文件）'
    )
    parser.add_argument(
        '--errors-only',
        action='store_true',
        help='仅显示错误'
    )
    parser.add_argument(
        '--base-path',
        default=os.path.dirname(os.path.abspath(__file__)),
        help='规则文件基础路径'
    )

    args = parser.parse_args()
    base_path = args.base_path

    if args.files:
        files = []
        for f in args.files:
            if os.path.isabs(f):
                files.append(f)
            else:
                files.append(os.path.join(base_path, f))
    else:
        files = find_rule_files(base_path)

    if not files:
        print("未找到规则文件")
        sys.exit(1)

    print("检查规则文件...")

    total_errors = []
    files_with_errors = 0
    files_passed = 0

    for file_path in files:
        if not os.path.exists(file_path):
            print(f"文件不存在: {file_path}", file=sys.stderr)
            continue

        errors = check_file(file_path)

        if errors:
            files_with_errors += 1
            total_errors.extend(errors)
            for error in errors:
                print_error(error, base_path)
        else:
            files_passed += 1
            if not args.errors_only:
                rel_path = os.path.relpath(file_path, base_path)
                print(f"✓ {rel_path}")

    print("\n" + "─" * 50)
    print(f"检查完成: {len(files)} 个文件")
    print(f"  ✓ 通过: {files_passed} 个")
    print(f"  ✗ 错误: {files_with_errors} 个 ({len(total_errors)} 处问题)")

    sys.exit(0 if not total_errors else 1)


if __name__ == '__main__':
    main()
