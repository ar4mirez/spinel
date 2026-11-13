//! RubyGems' own tests of `Gem::Platform` as tables, then two tables recorded
//! from real RubyGems.
//!
//! Copied from RubyGems, https://github.com/ruby/rubygems, at commit
//! c6c69d31d43a292ce4f3e6c75fd2769f02c436a9: `test/rubygems/test_gem_platform.rb`.
//! Each test keeps its upstream name and cases; `util_set_arch` is
//! [`Platform::local`]. A case that is about Ruby objects rather than platforms
//! (`inspect`, pattern matching, overriding `match_gem?`, a `Gem::Specification`
//! around the platform, `RUBY_SO_NAME`) is left out, and so is
//! `sort_and_filter_best_platform_match`, which is not ported.
//!
//! RubyGems is copyrighted free software by Chad Fowler, Rich Kilmer, Jim
//! Weirich and others, and the copied cases are used under its MIT license:
//!
//! > Copyright (c) Chad Fowler, Rich Kilmer, Jim Weirich and others.
//! > Portions copyright (c) 2010-2019 André Arko
//! > Portions copyright (c) 2009 Engine Yard
//! >
//! > Permission is hereby granted, free of charge, to any person obtaining
//! > a copy of this software and associated documentation files (the
//! > "Software"), to deal in the Software without restriction, including
//! > without limitation the rights to use, copy, modify, merge, publish,
//! > distribute, sublicense, and/or sell copies of the Software, and to
//! > permit persons to whom the Software is furnished to do so, subject to
//! > the following conditions:
//! >
//! > The above copyright notice and this permission notice shall be
//! > included in all copies or substantial portions of the Software.
//! >
//! > THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND,
//! > EXPRESS OR IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF
//! > MERCHANTABILITY, FITNESS FOR A PARTICULAR PURPOSE AND
//! > NONINFRINGEMENT. IN NO EVENT SHALL THE AUTHORS OR COPYRIGHT HOLDERS BE
//! > LIABLE FOR ANY CLAIM, DAMAGES OR OTHER LIABILITY, WHETHER IN AN ACTION
//! > OF CONTRACT, TORT OR OTHERWISE, ARISING FROM, OUT OF OR IN CONNECTION
//! > WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE SOFTWARE.
//!
//! The last two tables, `REAL_MATCHES` and `SPINEL`, are not RubyGems': they
//! were recorded from `Gem::Platform#===` and `Gem::Platform.match_platforms?`
//! on ruby 4.0.9 (RubyGems 4.0.20).

use super::*;

fn p(arch: &str) -> Platform {
    Platform::parse(arch)
}

fn parts(cpu: Option<&str>, os: &str, version: Option<&str>) -> Platform {
    Platform::from_parts(cpu, os, version)
}

fn gp(arch: &str) -> GemPlatform {
    GemPlatform::Platform(p(arch))
}

#[test]
fn test_self_local() {
    assert_eq!(
        Platform::local("i686-darwin8.10.1"),
        parts(Some("x86"), "darwin", Some("8"))
    );
    // `Gem::Platform.local` appends `_60` to a bare mswin arch.
    assert_eq!(
        Platform::local("i386-mswin32"),
        parts(Some("x86"), "mswin32", Some("60"))
    );
}

#[test]
fn test_self_match_gem() {
    let local = Platform::local("x86_64-linux");
    let platforms = [GemPlatform::Ruby, GemPlatform::Platform(local.clone())];
    let new = |arch: Option<&str>| GemPlatform::new(arch, &local);
    assert!(match_platforms(&new(None), &platforms), "nil == ruby");
    assert!(
        match_platforms(&GemPlatform::Platform(local.clone()), &platforms),
        "exact match"
    );
    assert!(
        match_platforms(&new(Some(&local.to_string())), &platforms),
        "=~ match"
    );
    assert!(match_platforms(&new(Some("ruby")), &platforms), "ruby");
}

#[test]
fn test_self_new() {
    let local = Platform::local("x86_64-linux");
    assert_eq!(
        GemPlatform::new(Some("current"), &local),
        GemPlatform::Platform(local.clone())
    );
    assert_eq!(GemPlatform::new(Some("ruby"), &local), GemPlatform::Ruby);
    assert_eq!(GemPlatform::new(None, &local), GemPlatform::Ruby);
    assert_eq!(GemPlatform::new(Some(""), &local), GemPlatform::Ruby);
}

type Parts<'a> = (Option<&'a str>, &'a str, Option<&'a str>);

const INITIALIZE: &[(&str, Parts)] = &[
    ("amd64-freebsd6", (Some("amd64"), "freebsd", Some("6"))),
    ("java", (None, "java", None)),
    ("jruby", (None, "java", None)),
    ("universal-dotnet", (Some("universal"), "dotnet", None)),
    (
        "universal-dotnet2.0",
        (Some("universal"), "dotnet", Some("2.0")),
    ),
    (
        "universal-dotnet4.0",
        (Some("universal"), "dotnet", Some("4.0")),
    ),
    ("powerpc-aix5.3.0.0", (Some("powerpc"), "aix", Some("5"))),
    ("powerpc-darwin7", (Some("powerpc"), "darwin", Some("7"))),
    ("powerpc-darwin8", (Some("powerpc"), "darwin", Some("8"))),
    ("powerpc-linux", (Some("powerpc"), "linux", None)),
    ("powerpc64-linux", (Some("powerpc64"), "linux", None)),
    (
        "sparc-solaris2.10",
        (Some("sparc"), "solaris", Some("2.10")),
    ),
    ("sparc-solaris2.8", (Some("sparc"), "solaris", Some("2.8"))),
    ("sparc-solaris2.9", (Some("sparc"), "solaris", Some("2.9"))),
    (
        "universal-darwin8",
        (Some("universal"), "darwin", Some("8")),
    ),
    (
        "universal-darwin9",
        (Some("universal"), "darwin", Some("9")),
    ),
    ("universal-macruby", (Some("universal"), "macruby", None)),
    ("i386-cygwin", (Some("x86"), "cygwin", None)),
    ("i686-darwin", (Some("x86"), "darwin", None)),
    ("i686-darwin8.4.1", (Some("x86"), "darwin", Some("8"))),
    ("i386-freebsd4.11", (Some("x86"), "freebsd", Some("4"))),
    ("i386-freebsd5", (Some("x86"), "freebsd", Some("5"))),
    ("i386-freebsd6", (Some("x86"), "freebsd", Some("6"))),
    ("i386-freebsd7", (Some("x86"), "freebsd", Some("7"))),
    ("i386-freebsd", (Some("x86"), "freebsd", None)),
    ("universal-freebsd", (Some("universal"), "freebsd", None)),
    ("i386-java1.5", (Some("x86"), "java", Some("1.5"))),
    ("x86-java1.6", (Some("x86"), "java", Some("1.6"))),
    ("i386-java1.6", (Some("x86"), "java", Some("1.6"))),
    ("i686-linux", (Some("x86"), "linux", None)),
    ("i586-linux", (Some("x86"), "linux", None)),
    ("i486-linux", (Some("x86"), "linux", None)),
    ("i386-linux", (Some("x86"), "linux", None)),
    ("i586-linux-gnu", (Some("x86"), "linux", Some("gnu"))),
    ("i386-linux-gnu", (Some("x86"), "linux", Some("gnu"))),
    ("i386-mingw32", (Some("x86"), "mingw32", None)),
    ("x64-mingw-ucrt", (Some("x64"), "mingw", Some("ucrt"))),
    ("i386-mswin32", (Some("x86"), "mswin32", None)),
    ("i386-mswin32_80", (Some("x86"), "mswin32", Some("80"))),
    ("i386-mswin32-80", (Some("x86"), "mswin32", Some("80"))),
    ("x86-mswin32", (Some("x86"), "mswin32", None)),
    ("x86-mswin32_60", (Some("x86"), "mswin32", Some("60"))),
    ("x86-mswin32-60", (Some("x86"), "mswin32", Some("60"))),
    ("i386-netbsdelf", (Some("x86"), "netbsdelf", None)),
    ("i386-openbsd4.0", (Some("x86"), "openbsd", Some("4.0"))),
    ("i386-solaris2.10", (Some("x86"), "solaris", Some("2.10"))),
    ("i386-solaris2.8", (Some("x86"), "solaris", Some("2.8"))),
    ("mswin32", (Some("x86"), "mswin32", None)),
    ("x86_64-linux", (Some("x86_64"), "linux", None)),
    ("x86_64-linux-gnu", (Some("x86_64"), "linux", Some("gnu"))),
    ("x86_64-linux-musl", (Some("x86_64"), "linux", Some("musl"))),
    (
        "x86_64-linux-uclibc",
        (Some("x86_64"), "linux", Some("uclibc")),
    ),
    ("arm-linux-eabi", (Some("arm"), "linux", Some("eabi"))),
    ("arm-linux-gnueabi", (Some("arm"), "linux", Some("gnueabi"))),
    (
        "arm-linux-musleabi",
        (Some("arm"), "linux", Some("musleabi")),
    ),
    (
        "arm-linux-uclibceabi",
        (Some("arm"), "linux", Some("uclibceabi")),
    ),
    (
        "x86_64-openbsd3.9",
        (Some("x86_64"), "openbsd", Some("3.9")),
    ),
    (
        "x86_64-openbsd4.0",
        (Some("x86_64"), "openbsd", Some("4.0")),
    ),
    ("x86_64-openbsd", (Some("x86_64"), "openbsd", None)),
    ("wasm32-wasi", (Some("wasm32"), "wasi", None)),
    ("wasm32-wasip1", (Some("wasm32"), "wasi", None)),
    ("wasm32-wasip2", (Some("wasm32"), "wasi", None)),
    ("darwin-java-java", (Some("darwin"), "java", None)),
    ("linux-linux-linux", (Some("linux"), "linux", Some("linux"))),
    (
        "linux-linux-linux1.0",
        (Some("linux"), "linux", Some("linux1")),
    ),
    (
        "x86x86-1x86x86x86x861linuxx86x86",
        (Some("x86x86"), "linux", Some("x86x86")),
    ),
    ("freebsd0", (None, "freebsd", Some("0"))),
    ("darwin0", (None, "darwin", Some("0"))),
    ("darwin0---", (None, "darwin", Some("0"))),
    ("x86-linux-x8611.0l", (Some("x86"), "linux", Some("x8611"))),
    ("0-x86linuxx86---", (Some("0"), "linux", Some("x86"))),
    ("x86_64-macruby-x86", (Some("x86_64"), "macruby", None)),
    ("x86_64-dotnetx86", (Some("x86_64"), "dotnet", None)),
    ("x86_64-dalvik0", (Some("x86_64"), "dalvik", Some("0"))),
    ("x86_64-dotnet1.", (Some("x86_64"), "dotnet", Some("1"))),
    ("--", (None, "unknown", None)),
];

#[test]
fn test_initialize() {
    for &(arch, expected) in INITIALIZE {
        let platform = p(arch);
        assert_eq!(platform.to_a(), expected, "{arch:?}");
        let platform2 = p(&platform.to_string());
        assert_eq!(platform2.to_a(), expected, "{arch:?} => {platform2}");
    }
}

#[test]
fn test_initialize_command_line() {
    assert_eq!(p("i386-mswin32").to_a(), (Some("x86"), "mswin32", None));
    assert_eq!(
        p("i386-mswin32-80").to_a(),
        (Some("x86"), "mswin32", Some("80"))
    );
    assert_eq!(
        p("i386-solaris-2.10").to_a(),
        (Some("x86"), "solaris", Some("2.10"))
    );
}

#[test]
fn test_initialize_platform() {
    let platform = p("cpu-my_platform1");
    assert_eq!(platform.cpu(), Some("cpu"));
    assert_eq!(platform.os(), "my_platform");
    assert_eq!(platform.version(), Some("1"));
}

#[test]
fn test_initialize_test() {
    let platform = p("cpu-my_platform1");
    assert_eq!(platform.to_a(), (Some("cpu"), "my_platform", Some("1")));
    let platform = p("cpu-other_platform1");
    assert_eq!(platform.to_a(), (Some("cpu"), "other_platform", Some("1")));
}

#[test]
fn test_to_s() {
    assert_eq!(
        Platform::local("i686-darwin8.10.1").to_string(),
        "x86-darwin-8"
    );
    assert_eq!(
        Platform::local("i386-mswin32").to_string(),
        "x86-mswin32-60"
    );
}

#[test]
fn test_equals2() {
    let my = parts(Some("cpu"), "my_platform", Some("1"));
    let other = parts(Some("cpu"), "other_platform", Some("1"));
    assert_eq!(my, my);
    assert_ne!(my, other);
    assert_ne!(other, my);
}

#[test]
fn test_equals3() {
    let my = parts(Some("cpu"), "my_platform", Some("1"));
    let other = parts(Some("cpu"), "other_platform", Some("1"));
    assert!(my.matches(&my));
    assert!(!other.matches(&my));
    assert!(!my.matches(&other));
}

#[test]
fn test_equals3_cpu() {
    let ppc_darwin8 = p("powerpc-darwin8.0");
    let uni_darwin8 = p("universal-darwin8.0");
    let x86_darwin8 = p("i686-darwin8.0");

    let local = Platform::local("powerpc-darwin8");
    assert!(ppc_darwin8.matches(&local), "powerpc =~ universal");
    assert!(uni_darwin8.matches(&local), "powerpc =~ universal");
    assert!(!x86_darwin8.matches(&local), "powerpc =~ universal");

    let local = Platform::local("i686-darwin8");
    assert!(!ppc_darwin8.matches(&local), "powerpc =~ universal");
    assert!(uni_darwin8.matches(&local), "x86 =~ universal");
    assert!(x86_darwin8.matches(&local), "powerpc =~ universal");

    let local = Platform::local("universal-darwin8");
    assert!(ppc_darwin8.matches(&local), "universal =~ ppc");
    assert!(uni_darwin8.matches(&local), "universal =~ universal");
    assert!(x86_darwin8.matches(&local), "universal =~ x86");
}

#[test]
fn test_nil_cpu_arch_is_treated_as_universal() {
    let with_nil_arch = parts(None, "mingw32", None);
    let with_uni_arch = parts(Some("universal"), "mingw32", None);
    let with_x86_arch = parts(Some("x86"), "mingw32", None);
    assert!(with_nil_arch.matches(&with_uni_arch), "nil =~ universal");
    assert!(with_uni_arch.matches(&with_nil_arch), "universal =~ nil");
    assert!(with_nil_arch.matches(&with_x86_arch), "nil =~ x86");
    assert!(with_x86_arch.matches(&with_nil_arch), "x86 =~ nil");
}

#[test]
fn test_nil_version_is_treated_as_any_version() {
    let x86_darwin_8 = p("i686-darwin8.0");
    let x86_darwin_nil = p("i686-darwin");
    assert!(x86_darwin_8.matches(&x86_darwin_nil), "8.0 =~ nil");
    assert!(x86_darwin_nil.matches(&x86_darwin_8), "nil =~ 8.0");
}

#[test]
fn test_nil_version_is_stricter_for_linux_os() {
    let x86_linux = p("i686-linux");
    let x86_linux_gnu = p("i686-linux-gnu");
    let x86_linux_musl = p("i686-linux-musl");
    let x86_linux_uclibc = p("i686-linux-uclibc");

    // a naked linux runtime is implicit gnu, as it represents the common glibc-linked runtime
    assert!(x86_linux.matches(&x86_linux_gnu), "linux =~ linux-gnu");
    assert!(x86_linux_gnu.matches(&x86_linux), "linux-gnu =~ linux");

    // musl and explicit gnu should differ
    assert!(
        !x86_linux_gnu.matches(&x86_linux_musl),
        "linux-gnu =~ linux-musl"
    );
    assert!(
        !x86_linux_musl.matches(&x86_linux_gnu),
        "linux-musl =~ linux-gnu"
    );

    // explicit libc differ
    assert!(
        !x86_linux_uclibc.matches(&x86_linux_musl),
        "linux-uclibc =~ linux-musl"
    );
    assert!(
        !x86_linux_musl.matches(&x86_linux_uclibc),
        "linux-musl =~ linux-uclibc"
    );

    // musl host runtime accepts libc-generic or statically linked gems...
    assert!(x86_linux.matches(&x86_linux_musl), "linux =~ linux-musl");
    // ...but implicit gnu runtime generally does not accept musl-specific gems
    assert!(!x86_linux_musl.matches(&x86_linux), "linux-musl =~ linux");

    // other libc are not glibc compatible
    assert!(
        !x86_linux.matches(&x86_linux_uclibc),
        "linux =~ linux-uclibc"
    );
    assert!(
        !x86_linux_uclibc.matches(&x86_linux),
        "linux-uclibc =~ linux"
    );
}

#[test]
fn test_eabi_version_is_stricter_for_linux_os() {
    let arm_linux_eabi = p("arm-linux-eabi");
    let arm_linux_gnueabi = p("arm-linux-gnueabi");
    let arm_linux_musleabi = p("arm-linux-musleabi");
    let arm_linux_uclibceabi = p("arm-linux-uclibceabi");

    // a naked linux runtime is implicit gnu, as it represents the common glibc-linked runtime
    assert!(
        arm_linux_eabi.matches(&arm_linux_gnueabi),
        "linux-eabi =~ linux-gnueabi"
    );
    assert!(
        arm_linux_gnueabi.matches(&arm_linux_eabi),
        "linux-gnueabi =~ linux-eabi"
    );

    // musl and explicit gnu should differ
    assert!(
        !arm_linux_gnueabi.matches(&arm_linux_musleabi),
        "linux-gnueabi =~ linux-musleabi"
    );
    assert!(
        !arm_linux_musleabi.matches(&arm_linux_gnueabi),
        "linux-musleabi =~ linux-gnueabi"
    );

    // explicit libc differ
    assert!(
        !arm_linux_uclibceabi.matches(&arm_linux_musleabi),
        "linux-uclibceabi =~ linux-musleabi"
    );
    assert!(
        !arm_linux_musleabi.matches(&arm_linux_uclibceabi),
        "linux-musleabi =~ linux-uclibceabi"
    );

    // musl host runtime accepts libc-generic or statically linked gems...
    assert!(
        arm_linux_eabi.matches(&arm_linux_musleabi),
        "linux-eabi =~ linux-musleabi"
    );
    // ...but implicit gnu runtime generally does not accept musl-specific gems
    assert!(
        !arm_linux_musleabi.matches(&arm_linux_eabi),
        "linux-musleabi =~ linux-eabi"
    );

    // other libc are not glibc compatible
    assert!(
        !arm_linux_eabi.matches(&arm_linux_uclibceabi),
        "linux-eabi =~ linux-uclibceabi"
    );
    assert!(
        !arm_linux_uclibceabi.matches(&arm_linux_eabi),
        "linux-uclibceabi =~ linux-eabi"
    );
}

#[test]
fn test_eabi_and_nil_version_combination_strictness() {
    let arm_linux = p("arm-linux");
    // generic arm host runtime with eabi modifier accepts generic arm gems
    assert!(
        arm_linux.matches(&p("arm-linux-eabi")),
        "arm-linux =~ arm-linux-eabi"
    );
    assert!(
        arm_linux.matches(&p("arm-linux-eabihf")),
        "arm-linux =~ arm-linux-eabihf"
    );

    // explicit gnu arm host runtime with eabi modifier accepts generic arm gems
    assert!(
        arm_linux.matches(&p("arm-linux-gnueabi")),
        "arm-linux =~ arm-linux-gnueabi"
    );
    assert!(
        arm_linux.matches(&p("arm-linux-gnueabihf")),
        "arm-linux =~ arm-linux-gnueabihf"
    );

    // musl arm host runtime accepts libc-generic or statically linked gems...
    assert!(
        arm_linux.matches(&p("arm-linux-musleabi")),
        "arm-linux =~ arm-linux-musleabi"
    );
    assert!(
        arm_linux.matches(&p("arm-linux-musleabihf")),
        "arm-linux =~ arm-linux-musleabihf"
    );

    // other libc arm hosts are not glibc compatible
    assert!(
        !arm_linux.matches(&p("arm-linux-uclibceabi")),
        "arm-linux =~ arm-linux-uclibceabi"
    );
    assert!(
        !arm_linux.matches(&p("arm-linux-uclibceabihf")),
        "arm-linux =~ arm-linux-uclibceabihf"
    );
}

#[test]
fn test_equals3_cpu_arm() {
    let arm = p("arm-linux");
    let armv5 = p("armv5-linux");
    let armv7 = p("armv7-linux");
    let arm64 = p("arm64-linux");

    let local = Platform::local("armv5-linux");
    assert!(arm.matches(&local), "arm   === armv5");
    assert!(armv5.matches(&local), "armv5 === armv5");
    assert!(!armv7.matches(&local), "armv7 === armv5");
    assert!(!arm64.matches(&local), "arm64 === armv5");
    assert!(!local.matches(&arm), "armv5 === arm");

    let local = Platform::local("armv7-linux");
    assert!(arm.matches(&local), "arm   === armv7");
    assert!(!armv5.matches(&local), "armv5 === armv7");
    assert!(armv7.matches(&local), "armv7 === armv7");
    assert!(!arm64.matches(&local), "arm64 === armv7");
    assert!(!local.matches(&arm), "armv7 === arm");

    let local = Platform::local("arm64-linux");
    assert!(!arm.matches(&local), "arm   === arm64");
    assert!(!armv5.matches(&local), "armv5 === arm64");
    assert!(!armv7.matches(&local), "armv7 === arm64");
    assert!(arm64.matches(&local), "arm64 === arm64");
}

#[test]
fn test_equals3_universal_mingw() {
    let uni_mingw = p("universal-mingw");
    let mingw_ucrt = p("x64-mingw-ucrt");
    let local = Platform::local("x64-mingw-ucrt");
    assert!(uni_mingw.matches(&local), "uni_mingw === mingw_ucrt");
    assert!(mingw_ucrt.matches(&local), "mingw_ucrt === mingw_ucrt");
}

#[test]
fn test_equals3_version() {
    let local = Platform::local("i686-darwin8");
    let x86_darwin = parts(Some("x86"), "darwin", None);
    let x86_darwin7 = parts(Some("x86"), "darwin", Some("7"));
    let x86_darwin8 = parts(Some("x86"), "darwin", Some("8"));
    let x86_darwin9 = parts(Some("x86"), "darwin", Some("9"));
    assert!(x86_darwin.matches(&local), "x86_darwin === x86_darwin8");
    assert!(x86_darwin8.matches(&local), "x86_darwin8 === x86_darwin8");
    assert!(!x86_darwin7.matches(&local), "x86_darwin7 === x86_darwin8");
    assert!(!x86_darwin9.matches(&local), "x86_darwin9 === x86_darwin8");
}

/// `assert_local_match` / `refute_local_match`: `Gem::Platform.local =~ name`.
const EQUALS_TILDE: &[(&str, &[&str], &[&str])] = &[
    (
        "i386-mswin32",
        &[
            "mswin32",
            "i386-mswin32",
            "i386-mswin32-mq5.3",
            "i386-mswin32-mq6",
        ],
        &[
            "win32-1.8.2-VC7",
            "win32-1.8.4-VC6",
            "win32-source",
            "windows",
        ],
    ),
    (
        "i686-linux",
        &["i486-linux", "i586-linux", "i686-linux"],
        &[],
    ),
    (
        "i686-darwin8",
        &["i686-darwin8.4.1", "i686-darwin8.8.2"],
        &[],
    ),
    ("java", &["java", "jruby"], &[]),
    (
        "universal-dotnet2.0",
        &[
            "universal-dotnet",
            "universal-dotnet-2.0",
            "dotnet",
            "dotnet-2.0",
        ],
        &["universal-dotnet-4.0", "dotnet-4.0"],
    ),
    (
        "universal-dotnet4.0",
        &[
            "universal-dotnet",
            "universal-dotnet-4.0",
            "dotnet",
            "dotnet-4.0",
        ],
        &["universal-dotnet-2.0", "dotnet-2.0"],
    ),
    (
        "universal-macruby-1.0",
        &["universal-macruby", "macruby", "universal-macruby-1.0"],
        &["universal-macruby-0.10"],
    ),
    ("powerpc-darwin", &["powerpc-darwin"], &[]),
    ("powerpc-darwin7", &["powerpc-darwin7.9.0"], &[]),
    ("powerpc-darwin8", &["powerpc-darwin8.10.0"], &[]),
    ("sparc-solaris2.8", &["sparc-solaris2.8-mq5.3"], &[]),
];

#[test]
fn test_equals_tilde() {
    for &(arch, matching, not_matching) in EQUALS_TILDE {
        let local = Platform::local(arch);
        for name in matching {
            assert!(local.matches_str(name, &local), "{local} =~ {name:?}");
        }
        for name in not_matching {
            assert!(!local.matches_str(name, &local), "{local} !~ {name:?}");
        }
    }
}

#[test]
fn test_constants() {
    assert_eq!(p("java").to_a(), (None, "java", None));
    assert_eq!(p("mswin32").to_a(), (Some("x86"), "mswin32", None));
    assert_eq!(p("mswin64").to_a(), (None, "mswin64", None));
    assert_eq!(p("x86-mingw32").to_a(), (Some("x86"), "mingw32", None));
    assert_eq!(
        p("x64-mingw-ucrt").to_a(),
        (Some("x64"), "mingw", Some("ucrt"))
    );
    assert_eq!(
        p("universal-mingw").to_a(),
        (Some("universal"), "mingw", None)
    );
    assert_eq!(p("x86_64-linux").to_a(), (Some("x86_64"), "linux", None));
    assert_eq!(
        p("x86_64-linux-musl").to_a(),
        (Some("x86_64"), "linux", Some("musl"))
    );
}

#[test]
fn test_generic() {
    let java = gp("java");
    let mswin = gp("mswin32");
    let mswin64 = gp("mswin64");
    let universal_mingw = gp("universal-mingw");

    // converts non-windows platforms into ruby
    assert_eq!(generic(&gp("x86-darwin-10")), GemPlatform::Ruby);
    assert_eq!(generic(&GemPlatform::Ruby), GemPlatform::Ruby);

    // converts java platform variants into java
    assert_eq!(generic(&gp("java")), java);
    assert_eq!(generic(&gp("universal-java-17")), java);

    // converts mswin platform variants into x86-mswin32
    assert_eq!(generic(&gp("mswin32")), mswin);
    assert_eq!(generic(&gp("i386-mswin32")), mswin);
    assert_eq!(generic(&gp("x86-mswin32")), mswin);

    // converts 32-bit mingw platform variants into universal-mingw
    assert_eq!(generic(&gp("i386-mingw32")), universal_mingw);
    assert_eq!(generic(&gp("x86-mingw32")), universal_mingw);

    // converts 64-bit mingw platform variants into universal-mingw
    assert_eq!(generic(&gp("x64-mingw32")), universal_mingw);

    // converts x64 mingw UCRT platform variants into universal-mingw
    assert_eq!(generic(&gp("x64-mingw-ucrt")), universal_mingw);

    // converts aarch64 mingw UCRT platform variants into universal-mingw
    assert_eq!(generic(&gp("aarch64-mingw-ucrt")), universal_mingw);

    assert_eq!(generic(&gp("unknown")), GemPlatform::Ruby);
    assert_eq!(generic(&gp("mswin64")), mswin64);
}

#[test]
fn test_platform_specificity_match() {
    let local = Platform::local("x86_64-linux");
    let new = |arch: &str| GemPlatform::new(Some(arch), &local);
    for &(spec, user, s1, s2) in &[
        ("ruby", "ruby", -1, -1),
        ("x86_64-linux-musl", "x86_64-linux-musl", -1, -1),
        ("x86_64-linux", "x86_64-linux-musl", 100, 200),
        ("universal-darwin", "x86-darwin", 10, 20),
        ("universal-darwin-19", "x86-darwin", 210, 120),
        ("universal-darwin-19", "universal-darwin-20", 200, 200),
        ("arm-darwin-19", "arm64-darwin-19", 0, 20),
    ] {
        let (spec, user) = (new(spec), new(user));
        assert_eq!(
            platform_specificity_match(&spec, &user),
            s1,
            "({spec}, {user})"
        );
        assert_eq!(
            platform_specificity_match(&user, &spec),
            s2,
            "({user}, {spec})"
        );
    }
}

#[test]
fn test_sort_best_platform_match() {
    let local = Platform::local("x86_64-linux");
    let new = |arch: &str| GemPlatform::new(Some(arch), &local);
    let specs = [
        "ruby",
        "java",
        "universal-darwin",
        "universal-darwin-19",
        "universal-darwin-20",
        "arm64-darwin-19",
        "x86-darwin",
    ]
    .map(new);
    let sorted = |platform: &str| {
        let mut specs = specs.clone();
        sort_best_platform_match(&mut specs, &new(platform), |s| s);
        specs.iter().map(ToString::to_string).collect::<Vec<_>>()
    };
    assert_eq!(
        sorted("ruby"),
        [
            "ruby",
            "java",
            "universal-darwin",
            "universal-darwin-19",
            "universal-darwin-20",
            "arm64-darwin-19",
            "x86-darwin"
        ]
    );
    assert_eq!(
        sorted("java"),
        [
            "java",
            "universal-darwin",
            "x86-darwin",
            "universal-darwin-19",
            "universal-darwin-20",
            "arm64-darwin-19",
            "ruby"
        ]
    );
    assert_eq!(
        sorted("arm64-darwin-19"),
        [
            "arm64-darwin-19",
            "universal-darwin-19",
            "universal-darwin",
            "java",
            "x86-darwin",
            "universal-darwin-20",
            "ruby"
        ]
    );
    assert_eq!(
        sorted("arm64-darwin-20"),
        [
            "universal-darwin-20",
            "universal-darwin",
            "java",
            "x86-darwin",
            "arm64-darwin-19",
            "universal-darwin-19",
            "ruby"
        ]
    );
    assert_eq!(
        sorted("x86-darwin-19"),
        [
            "universal-darwin-19",
            "arm64-darwin-19",
            "x86-darwin",
            "universal-darwin",
            "java",
            "universal-darwin-20",
            "ruby"
        ]
    );
    assert_eq!(
        sorted("x86-darwin-20"),
        [
            "universal-darwin-20",
            "x86-darwin",
            "universal-darwin",
            "java",
            "universal-darwin-19",
            "arm64-darwin-19",
            "ruby"
        ]
    );
    assert_eq!(
        sorted("x86-darwin-21"),
        [
            "x86-darwin",
            "universal-darwin",
            "java",
            "universal-darwin-19",
            "universal-darwin-20",
            "arm64-darwin-19",
            "ruby"
        ]
    );
}

#[test]
fn test_deconstruct() {
    assert_eq!(p("x86_64-linux").to_a(), (Some("x86_64"), "linux", None));
    assert_eq!(
        p("x86_64-darwin-20").to_a(),
        (Some("x86_64"), "darwin", Some("20"))
    );
}

// The tables below are not RubyGems': recorded from ruby 4.0.9 (RubyGems 4.0.20).

/// Common gem platforms on rubygems.org against common runtimes: for each gem
/// platform, the runtimes `Gem::Platform#===` lets it install on.
const REAL_MATCHES: &[(&str, &[&str])] = &[
    (
        "x86_64-linux",
        &["x86_64-linux", "x86_64-linux-gnu", "x86_64-linux-musl"],
    ),
    ("x86_64-linux-gnu", &["x86_64-linux", "x86_64-linux-gnu"]),
    ("x86_64-linux-musl", &["x86_64-linux-musl"]),
    ("aarch64-linux", &["aarch64-linux", "aarch64-linux-musl"]),
    ("aarch64-linux-gnu", &["aarch64-linux"]),
    ("aarch64-linux-musl", &["aarch64-linux-musl"]),
    (
        "arm-linux",
        &[
            "arm-linux-gnueabihf",
            "arm-linux-musleabihf",
            "armv7-linux-gnueabihf",
        ],
    ),
    (
        "arm-linux-gnueabihf",
        &["arm-linux-gnueabihf", "armv7-linux-gnueabihf"],
    ),
    ("arm-linux-musleabihf", &["arm-linux-musleabihf"]),
    ("armv7-linux-gnueabihf", &["armv7-linux-gnueabihf"]),
    ("arm64-darwin", &["arm64-darwin-22", "arm64-darwin-24"]),
    ("arm64-darwin-22", &["arm64-darwin-22"]),
    ("arm64-darwin-24", &["arm64-darwin-24"]),
    ("x86_64-darwin", &["x86_64-darwin-19"]),
    ("x86_64-darwin-19", &["x86_64-darwin-19"]),
    (
        "universal-darwin",
        &["arm64-darwin-22", "arm64-darwin-24", "x86_64-darwin-19"],
    ),
    ("universal-darwin-20", &[]),
    ("x64-mingw-ucrt", &["x64-mingw-ucrt"]),
    ("x64-mingw32", &["x64-mingw32"]),
    ("x86-mingw32", &["x86-mingw32"]),
    (
        "universal-mingw",
        &[
            "x64-mingw-ucrt",
            "x64-mingw32",
            "x86-mingw32",
            "aarch64-mingw-ucrt",
        ],
    ),
    ("aarch64-mingw-ucrt", &["aarch64-mingw-ucrt"]),
    ("x86-mswin32", &[]),
    ("x64-mswin64-140", &["x64-mswin64_140"]),
    ("java", &["universal-java-17", "java"]),
    ("universal-java", &["universal-java-17", "java"]),
    ("universal-java-17", &["universal-java-17", "java"]),
    ("x86_64-freebsd", &["x86_64-freebsd-14"]),
    ("x86_64-freebsd-14", &["x86_64-freebsd-14"]),
    ("x86_64-openbsd", &["x86_64-openbsd-7.5"]),
    ("x86_64-solaris-2.11", &["x86_64-solaris-2.11"]),
    ("wasm32-wasi", &["wasm32-wasi"]),
    ("x86-linux", &["i686-linux"]),
    ("i686-linux", &["i686-linux"]),
];

const RUNTIMES: &[&str] = &[
    "x86_64-linux",
    "x86_64-linux-gnu",
    "x86_64-linux-musl",
    "aarch64-linux",
    "aarch64-linux-musl",
    "arm-linux-gnueabihf",
    "arm-linux-musleabihf",
    "armv7-linux-gnueabihf",
    "arm64-darwin-22",
    "arm64-darwin-24",
    "x86_64-darwin-19",
    "x64-mingw-ucrt",
    "x64-mingw32",
    "x86-mingw32",
    "aarch64-mingw-ucrt",
    "x64-mswin64_140",
    "universal-java-17",
    "java",
    "x86_64-freebsd-14",
    "x86_64-openbsd-7.5",
    "x86_64-solaris-2.11",
    "wasm32-wasi",
    "i686-linux",
];

#[test]
fn real_platforms_match_as_rubygems_does() {
    for &(gem, matching) in REAL_MATCHES {
        let found: Vec<&str> = RUNTIMES
            .iter()
            .copied()
            .filter(|r| p(gem).matches(&p(r)))
            .collect();
        assert_eq!(found, matching, "{gem} === runtimes");
    }
}

/// `<cpu>-<os>-spinel`, the platform docs/package-manager.md (section 6) names
/// for Spinel-native gems: how RubyGems reads it, what `gem build` names it
/// (`to_s`), and which runtimes' `Gem.platforms` (`ruby` and the local
/// platform) accept it. RubyGems must accept none for the scheme to be safe;
/// only the linux and mingw spellings without a libc pass. Every darwin one is
/// read as plain `<cpu>-darwin` and installs on every version of macOS.
const SPINEL: &[(&str, Parts, &str, &[&str])] = &[
    (
        "x86_64-linux-spinel",
        (Some("x86_64"), "linux", Some("spinel")),
        "x86_64-linux-spinel",
        &[],
    ),
    (
        "x86_64-linux-gnu-spinel",
        (Some("x86_64"), "linux", Some("gnu")),
        "x86_64-linux-gnu",
        &["x86_64-linux", "x86_64-linux-gnu"],
    ),
    (
        "x86_64-linux-musl-spinel",
        (Some("x86_64"), "linux", Some("musl")),
        "x86_64-linux-musl",
        &["x86_64-linux-musl"],
    ),
    (
        "x86_64-darwin-spinel",
        (Some("x86_64"), "darwin", None),
        "x86_64-darwin",
        &["x86_64-darwin-21", "x86_64-darwin-23"],
    ),
    (
        "x86_64-freebsd-spinel",
        (Some("x86_64"), "freebsd", None),
        "x86_64-freebsd",
        &["x86_64-freebsd-14"],
    ),
    (
        "x86_64-openbsd-spinel",
        (Some("x86_64"), "openbsd", None),
        "x86_64-openbsd",
        &["x86_64-openbsd-7.5"],
    ),
    (
        "x86_64-mingw-spinel",
        (Some("x86_64"), "mingw", Some("spinel")),
        "x86_64-mingw-spinel",
        &[],
    ),
    (
        "x86_64-mingw32-spinel",
        (Some("x86_64"), "mingw32", None),
        "x86_64-mingw32",
        &[],
    ),
    (
        "x86_64-mswin64-spinel",
        (Some("x86_64"), "mswin64", None),
        "x86_64-mswin64",
        &[],
    ),
    (
        "x86_64-java-spinel",
        (Some("x86_64"), "java", None),
        "x86_64-java",
        &["java", "universal-java-21"],
    ),
    (
        "x86_64-solaris-spinel",
        (Some("x86_64"), "solaris", None),
        "x86_64-solaris",
        &["x86_64-solaris-2.11"],
    ),
    (
        "x86_64-aix-spinel",
        (Some("x86_64"), "aix", None),
        "x86_64-aix",
        &[],
    ),
    (
        "x86_64-cygwin-spinel",
        (Some("x86_64"), "cygwin", None),
        "x86_64-cygwin",
        &["x86_64-cygwin"],
    ),
    (
        "aarch64-linux-spinel",
        (Some("aarch64"), "linux", Some("spinel")),
        "aarch64-linux-spinel",
        &[],
    ),
    (
        "aarch64-linux-gnu-spinel",
        (Some("aarch64"), "linux", Some("gnu")),
        "aarch64-linux-gnu",
        &["aarch64-linux", "aarch64-linux-gnu"],
    ),
    (
        "aarch64-linux-musl-spinel",
        (Some("aarch64"), "linux", Some("musl")),
        "aarch64-linux-musl",
        &["aarch64-linux-musl"],
    ),
    (
        "aarch64-darwin-spinel",
        (Some("aarch64"), "darwin", None),
        "aarch64-darwin",
        &[],
    ),
    (
        "aarch64-freebsd-spinel",
        (Some("aarch64"), "freebsd", None),
        "aarch64-freebsd",
        &[],
    ),
    (
        "aarch64-openbsd-spinel",
        (Some("aarch64"), "openbsd", None),
        "aarch64-openbsd",
        &[],
    ),
    (
        "aarch64-mingw-spinel",
        (Some("aarch64"), "mingw", Some("spinel")),
        "aarch64-mingw-spinel",
        &[],
    ),
    (
        "aarch64-mingw32-spinel",
        (Some("aarch64"), "mingw32", None),
        "aarch64-mingw32",
        &[],
    ),
    (
        "aarch64-mswin64-spinel",
        (Some("aarch64"), "mswin64", None),
        "aarch64-mswin64",
        &[],
    ),
    (
        "aarch64-java-spinel",
        (Some("aarch64"), "java", None),
        "aarch64-java",
        &["java", "universal-java-21"],
    ),
    (
        "aarch64-solaris-spinel",
        (Some("aarch64"), "solaris", None),
        "aarch64-solaris",
        &[],
    ),
    (
        "aarch64-aix-spinel",
        (Some("aarch64"), "aix", None),
        "aarch64-aix",
        &[],
    ),
    (
        "aarch64-cygwin-spinel",
        (Some("aarch64"), "cygwin", None),
        "aarch64-cygwin",
        &[],
    ),
    (
        "arm64-linux-spinel",
        (Some("arm64"), "linux", Some("spinel")),
        "arm64-linux-spinel",
        &[],
    ),
    (
        "arm64-linux-gnu-spinel",
        (Some("arm64"), "linux", Some("gnu")),
        "arm64-linux-gnu",
        &[],
    ),
    (
        "arm64-linux-musl-spinel",
        (Some("arm64"), "linux", Some("musl")),
        "arm64-linux-musl",
        &[],
    ),
    (
        "arm64-darwin-spinel",
        (Some("arm64"), "darwin", None),
        "arm64-darwin",
        &[
            "arm64-darwin-22",
            "arm64-darwin-23",
            "arm64-darwin-24",
            "arm64-darwin-25",
        ],
    ),
    (
        "arm64-freebsd-spinel",
        (Some("arm64"), "freebsd", None),
        "arm64-freebsd",
        &[],
    ),
    (
        "arm64-openbsd-spinel",
        (Some("arm64"), "openbsd", None),
        "arm64-openbsd",
        &[],
    ),
    (
        "arm64-mingw-spinel",
        (Some("arm64"), "mingw", Some("spinel")),
        "arm64-mingw-spinel",
        &[],
    ),
    (
        "arm64-mingw32-spinel",
        (Some("arm64"), "mingw32", None),
        "arm64-mingw32",
        &[],
    ),
    (
        "arm64-mswin64-spinel",
        (Some("arm64"), "mswin64", None),
        "arm64-mswin64",
        &[],
    ),
    (
        "arm64-java-spinel",
        (Some("arm64"), "java", None),
        "arm64-java",
        &["java", "universal-java-21"],
    ),
    (
        "arm64-solaris-spinel",
        (Some("arm64"), "solaris", None),
        "arm64-solaris",
        &[],
    ),
    (
        "arm64-aix-spinel",
        (Some("arm64"), "aix", None),
        "arm64-aix",
        &[],
    ),
    (
        "arm64-cygwin-spinel",
        (Some("arm64"), "cygwin", None),
        "arm64-cygwin",
        &[],
    ),
    (
        "arm-linux-spinel",
        (Some("arm"), "linux", Some("spinel")),
        "arm-linux-spinel",
        &[],
    ),
    (
        "arm-linux-gnu-spinel",
        (Some("arm"), "linux", Some("gnu")),
        "arm-linux-gnu",
        &["arm-linux-gnueabihf"],
    ),
    (
        "arm-linux-musl-spinel",
        (Some("arm"), "linux", Some("musl")),
        "arm-linux-musl",
        &[],
    ),
    (
        "arm-darwin-spinel",
        (Some("arm"), "darwin", None),
        "arm-darwin",
        &[],
    ),
    (
        "arm-freebsd-spinel",
        (Some("arm"), "freebsd", None),
        "arm-freebsd",
        &[],
    ),
    (
        "arm-openbsd-spinel",
        (Some("arm"), "openbsd", None),
        "arm-openbsd",
        &[],
    ),
    (
        "arm-mingw-spinel",
        (Some("arm"), "mingw", Some("spinel")),
        "arm-mingw-spinel",
        &[],
    ),
    (
        "arm-mingw32-spinel",
        (Some("arm"), "mingw32", None),
        "arm-mingw32",
        &[],
    ),
    (
        "arm-mswin64-spinel",
        (Some("arm"), "mswin64", None),
        "arm-mswin64",
        &[],
    ),
    (
        "arm-java-spinel",
        (Some("arm"), "java", None),
        "arm-java",
        &["java", "universal-java-21"],
    ),
    (
        "arm-solaris-spinel",
        (Some("arm"), "solaris", None),
        "arm-solaris",
        &[],
    ),
    ("arm-aix-spinel", (Some("arm"), "aix", None), "arm-aix", &[]),
    (
        "arm-cygwin-spinel",
        (Some("arm"), "cygwin", None),
        "arm-cygwin",
        &[],
    ),
    (
        "x86-linux-spinel",
        (Some("x86"), "linux", Some("spinel")),
        "x86-linux-spinel",
        &[],
    ),
    (
        "x86-linux-gnu-spinel",
        (Some("x86"), "linux", Some("gnu")),
        "x86-linux-gnu",
        &[],
    ),
    (
        "x86-linux-musl-spinel",
        (Some("x86"), "linux", Some("musl")),
        "x86-linux-musl",
        &[],
    ),
    (
        "x86-darwin-spinel",
        (Some("x86"), "darwin", None),
        "x86-darwin",
        &[],
    ),
    (
        "x86-freebsd-spinel",
        (Some("x86"), "freebsd", None),
        "x86-freebsd",
        &[],
    ),
    (
        "x86-openbsd-spinel",
        (Some("x86"), "openbsd", None),
        "x86-openbsd",
        &[],
    ),
    (
        "x86-mingw-spinel",
        (Some("x86"), "mingw", Some("spinel")),
        "x86-mingw-spinel",
        &[],
    ),
    (
        "x86-mingw32-spinel",
        (Some("x86"), "mingw32", None),
        "x86-mingw32",
        &["x86-mingw32"],
    ),
    (
        "x86-mswin64-spinel",
        (Some("x86"), "mswin64", None),
        "x86-mswin64",
        &[],
    ),
    (
        "x86-java-spinel",
        (Some("x86"), "java", None),
        "x86-java",
        &["java", "universal-java-21"],
    ),
    (
        "x86-solaris-spinel",
        (Some("x86"), "solaris", None),
        "x86-solaris",
        &[],
    ),
    ("x86-aix-spinel", (Some("x86"), "aix", None), "x86-aix", &[]),
    (
        "x86-cygwin-spinel",
        (Some("x86"), "cygwin", None),
        "x86-cygwin",
        &[],
    ),
    (
        "x64-linux-spinel",
        (Some("x64"), "linux", Some("spinel")),
        "x64-linux-spinel",
        &[],
    ),
    (
        "x64-linux-gnu-spinel",
        (Some("x64"), "linux", Some("gnu")),
        "x64-linux-gnu",
        &[],
    ),
    (
        "x64-linux-musl-spinel",
        (Some("x64"), "linux", Some("musl")),
        "x64-linux-musl",
        &[],
    ),
    (
        "x64-darwin-spinel",
        (Some("x64"), "darwin", None),
        "x64-darwin",
        &[],
    ),
    (
        "x64-freebsd-spinel",
        (Some("x64"), "freebsd", None),
        "x64-freebsd",
        &[],
    ),
    (
        "x64-openbsd-spinel",
        (Some("x64"), "openbsd", None),
        "x64-openbsd",
        &[],
    ),
    (
        "x64-mingw-spinel",
        (Some("x64"), "mingw", Some("spinel")),
        "x64-mingw-spinel",
        &[],
    ),
    (
        "x64-mingw32-spinel",
        (Some("x64"), "mingw32", None),
        "x64-mingw32",
        &["x64-mingw32"],
    ),
    (
        "x64-mswin64-spinel",
        (Some("x64"), "mswin64", None),
        "x64-mswin64",
        &["x64-mswin64-140"],
    ),
    (
        "x64-java-spinel",
        (Some("x64"), "java", None),
        "x64-java",
        &["java", "universal-java-21"],
    ),
    (
        "x64-solaris-spinel",
        (Some("x64"), "solaris", None),
        "x64-solaris",
        &[],
    ),
    ("x64-aix-spinel", (Some("x64"), "aix", None), "x64-aix", &[]),
    (
        "x64-cygwin-spinel",
        (Some("x64"), "cygwin", None),
        "x64-cygwin",
        &[],
    ),
];

const LOCALS: &[&str] = &[
    "x86_64-linux",
    "x86_64-linux-gnu",
    "x86_64-linux-musl",
    "aarch64-linux",
    "aarch64-linux-gnu",
    "aarch64-linux-musl",
    "arm-linux-gnueabihf",
    "arm64-darwin-22",
    "arm64-darwin-23",
    "arm64-darwin-24",
    "arm64-darwin-25",
    "x86_64-darwin-21",
    "x86_64-darwin-23",
    "x64-mingw-ucrt",
    "x64-mingw32",
    "x86-mingw32",
    "x64-mswin64-140",
    "x86_64-freebsd-14",
    "x86_64-openbsd-7.5",
    "java",
    "universal-java-21",
    "x86_64-solaris-2.11",
    "powerpc-aix-7",
    "x86_64-cygwin",
];

#[test]
fn spinel_platforms_read_and_match_as_rubygems_does() {
    for &(arch, expected, to_s, matching) in SPINEL {
        let local = Platform::local("x86_64-linux");
        let platform = GemPlatform::new(Some(arch), &local);
        let GemPlatform::Platform(parsed) = &platform else {
            panic!("{arch} is ruby");
        };
        assert_eq!(parsed.to_a(), expected, "{arch}");
        assert_eq!(platform.to_string(), to_s, "{arch}");
        let found: Vec<&str> = LOCALS
            .iter()
            .copied()
            .filter(|l| match_platforms(&platform, &[GemPlatform::Ruby, gp(l)]))
            .collect();
        assert_eq!(found, matching, "{arch} on Gem.platforms");
    }
}

/// What makes a `<cpu>-<os>-spinel` spelling safe: `spinel` survives as the
/// version, which only the linux and mingw rules keep, and only straight after
/// the os (`linux-gnu-spinel` is `linux-gnu`) (`linux-?(\w+)?`,
/// `mingw-?(\w+)?`), and no runtime has that version. Everywhere else the
/// suffix is dropped and the gem is an ordinary `<cpu>-<os>` gem.
#[test]
fn a_spinel_suffix_survives_only_on_linux_and_mingw() {
    let local = Platform::local("x86_64-linux");
    for &(arch, (_, _, version), to_s, matching) in SPINEL {
        let (_, os) = arch.split_once('-').unwrap();
        let survives = version == Some("spinel");
        assert_eq!(
            survives,
            matches!(os, "linux-spinel" | "mingw-spinel"),
            "{arch}"
        );
        assert_eq!(survives, to_s == arch, "{arch} builds as {to_s}");
        if survives {
            assert!(matching.is_empty(), "{arch}");
            let runtimes = LOCALS.iter().chain(RUNTIMES).map(|r| Platform::local(r));
            for runtime in runtimes.chain([local.clone()]) {
                assert!(!p(arch).matches(&runtime), "{arch} === {runtime}");
            }
        }
    }
    let unsafe_count = SPINEL
        .iter()
        .filter(|(.., matching)| !matching.is_empty())
        .count();
    assert_eq!((unsafe_count, SPINEL.len()), (20, 78));
}

/// Lines and their ends: Ruby's `^` and `$` match at every newline, not only
/// at the ends of the string. Recorded from ruby 4.0.9.
#[test]
fn line_anchors_are_ruby_line_anchors() {
    assert_eq!(
        p("x86_64-foo\nmacruby-1.2").to_a(),
        (Some("x86_64"), "macruby", Some("1.2"))
    );
    assert_eq!(p("a-\nb-").to_a(), (Some("a\nb"), "unknown", None));
    assert_eq!(
        p("x86_64-linux-ab\ncd").to_a(),
        (Some("x86_64"), "linux", Some("ab"))
    );
    assert_eq!(
        p("cpu-x_platform_platform7").to_a(),
        (Some("cpu"), "x_platform_platform", Some("7"))
    );
    assert_eq!(p("-abc").to_a(), (Some(""), "unknown", None));
    assert_eq!(p("i686").to_a(), (None, "unknown", None));
    // `\w` and `\d` are ASCII.
    assert_eq!(p("x86_64-linux-é").to_a(), (Some("x86_64"), "linux", None));
}
