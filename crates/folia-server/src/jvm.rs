use anyhow::{Context, Result};
use folia_core::region::{ChunkPos, RegionScheduler};
use jni::objects::{GlobalRef, JClass, JObject, JString, JValue};
use jni::sys::{jint, jlong, jstring};
use jni::{InitArgsBuilder, JNIEnv, JNIVersion, JavaVM, NativeMethod};
use std::ffi::c_void;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicI32, Ordering};
use std::sync::{Arc, LazyLock, Mutex};
use std::time::Duration;

struct Task {
    id: i32,
    plugin: String,
    runnable: GlobalRef,
    delay: i64,
    period: i64,
    cancelled: bool,
}

// SAFETY: GlobalRef is a JVM global reference. It remains valid across threads;
// all calls that use it happen on the JVM-attached server thread.
unsafe impl Send for Task {}

static NEXT_TASK_ID: AtomicI32 = AtomicI32::new(1);
static TASKS: LazyLock<Mutex<Vec<Task>>> = LazyLock::new(|| Mutex::new(Vec::new()));

extern "system" fn native_ping(mut env: JNIEnv, _class: JClass, input: JString) -> jstring {
    let message: String = match env.get_string(&input) {
        Ok(value) => value.into(),
        Err(_) => "<invalid jstring>".to_owned(),
    };
    println!("[rust] nativePing received: {message}");
    let response = format!("Rust core received: {message}");
    match env.new_string(response) {
        Ok(value) => value.into_raw(),
        Err(_) => std::ptr::null_mut(),
    }
}

extern "system" fn native_schedule_task(
    mut env: JNIEnv,
    _class: JClass,
    plugin: JString,
    runnable: JObject,
    delay: jlong,
    period: jlong,
) -> jint {
    let plugin_name: String = match env.get_string(&plugin) {
        Ok(value) => value.into(),
        Err(_) => "<unknown plugin>".to_owned(),
    };
    let runnable = match env.new_global_ref(&runnable) {
        Ok(value) => value,
        Err(error) => {
            eprintln!("[rust] could not pin plugin task: {error}");
            return -1;
        }
    };
    let id = NEXT_TASK_ID.fetch_add(1, Ordering::SeqCst);
    let mut tasks = TASKS
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    tasks.push(Task {
        id,
        plugin: plugin_name.clone(),
        runnable,
        delay: delay.max(0),
        period: period.max(0),
        cancelled: false,
    });
    println!("[rust] scheduled task #{id} for {plugin_name} (delay={delay}, period={period})");
    id
}

extern "system" fn native_cancel_task(_env: JNIEnv, _class: JClass, id: jint) {
    let mut tasks = TASKS
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    for task in tasks.iter_mut() {
        if task.id == id {
            task.cancelled = true;
        }
    }
    tasks.retain(|task| !task.cancelled);
}

extern "system" fn native_cancel_tasks(mut env: JNIEnv, _class: JClass, plugin: JString) {
    let plugin_name: String = match env.get_string(&plugin) {
        Ok(value) => value.into(),
        Err(_) => return,
    };
    let mut tasks = TASKS
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    for task in tasks.iter_mut() {
        if task.plugin == plugin_name {
            task.cancelled = true;
        }
    }
    tasks.retain(|task| !task.cancelled);
}

fn tick_tasks(env: &mut JNIEnv) {
    let mut due = Vec::new();
    {
        let mut tasks = TASKS
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        for task in tasks.iter_mut() {
            if task.cancelled {
                continue;
            }
            if task.delay > 0 {
                task.delay -= 1;
                continue;
            }
            due.push((task.id, task.runnable.clone()));
            if task.period > 0 {
                task.delay = task.period;
            } else {
                task.cancelled = true;
            }
        }
        tasks.retain(|task| !task.cancelled);
    }

    for (id, runnable) in due {
        if let Err(error) = env.call_method(runnable.as_obj(), "run", "()V", &[]) {
            let _ = env.exception_describe();
            let _ = env.exception_clear();
            eprintln!("[rust] scheduled task #{id} threw: {error}");
        }
    }
}

pub fn run(root: &Path) -> Result<()> {
    eprintln!("[rust] locating JDK");
    ensure_java_home();
    eprintln!("[rust] building classpath");
    let classpath = build_classpath(root)?;
    eprintln!("[rust] classpath built ({} bytes)", classpath.len());
    eprintln!("[rust] bootstrapping JVM (root={})", root.display());

    let args = InitArgsBuilder::new()
        .version(JNIVersion::V8)
        .option("-Dfile.encoding=UTF-8")
        .option(format!("-Djava.class.path={classpath}"))
        .build()
        .context("failed to build JVM arguments")?;

    let jvm = JavaVM::new(args).context("failed to start the embedded JVM")?;
    eprintln!("[rust] JVM started");
    let mut env = jvm
        .attach_current_thread()
        .context("failed to attach to the embedded JVM")?;

    let bridge = env
        .find_class("rs/folia/bridge/Bridge")
        .context("bridge class is missing; run scripts/build-java.ps1 first")?;
    eprintln!("[rust] bridge class loaded");

    let methods = [
        NativeMethod {
            name: "nativePing".into(),
            sig: "(Ljava/lang/String;)Ljava/lang/String;".into(),
            fn_ptr: native_ping as *mut c_void,
        },
        NativeMethod {
            name: "nativeScheduleTask".into(),
            sig: "(Ljava/lang/String;Ljava/lang/Runnable;JJ)I".into(),
            fn_ptr: native_schedule_task as *mut c_void,
        },
        NativeMethod {
            name: "nativeCancelTask".into(),
            sig: "(I)V".into(),
            fn_ptr: native_cancel_task as *mut c_void,
        },
        NativeMethod {
            name: "nativeCancelTasks".into(),
            sig: "(Ljava/lang/String;)V".into(),
            fn_ptr: native_cancel_tasks as *mut c_void,
        },
    ];
    env.register_native_methods(&bridge, &methods)
        .context("failed to register native bridge methods")?;

    let plugins_dir = root.join("plugins");
    let data_dir = root.join("run");
    std::fs::create_dir_all(&plugins_dir)?;
    std::fs::create_dir_all(&data_dir)?;

    {
        let plugins = env.new_string(plugins_dir.to_string_lossy())?;
        let data = env.new_string(data_dir.to_string_lossy())?;
        let plugins = JObject::from(plugins);
        let data = JObject::from(data);
        call(
            &mut env,
            &bridge,
            "boot",
            "(Ljava/lang/String;Ljava/lang/String;)V",
            &[JValue::Object(&plugins), JValue::Object(&data)],
        )
        .context("plugin host boot failed")?;
        eprintln!("[rust] plugin host booted");
    }

    {
        let line = JObject::from(env.new_string("hello Rust")?);
        call(
            &mut env,
            &bridge,
            "dispatchCommand",
            "(Ljava/lang/String;)V",
            &[JValue::Object(&line)],
        )
        .context("plugin command dispatch failed")?;
    }

    let running = Arc::new(AtomicBool::new(true));
    {
        let running = Arc::clone(&running);
        ctrlc::set_handler(move || running.store(false, Ordering::SeqCst))
            .context("failed to install the Ctrl+C handler")?;
    }

    let max_ticks = std::env::var("FOLIA_MAX_TICKS")
        .ok()
        .and_then(|value| value.parse::<u64>().ok());

    match max_ticks {
        Some(limit) => println!("[rust] server loop running for at most {limit} ticks"),
        None => println!("[rust] server loop running (Ctrl+C to stop)"),
    }

    let mut regions = RegionScheduler::new(8);
    regions.schedule_at(ChunkPos::new(0, 0), 5, 0, || {
        println!("[rust] spawn region task fired");
    });
    regions.schedule_global(0, 100, || {
        println!("[rust] global region heartbeat");
    });

    let mut ticks = 0u64;
    while running.load(Ordering::SeqCst) {
        let region_tasks = regions.tick_parallel(4);
        if region_tasks > 0 {
            println!("[rust] region scheduler ran {region_tasks} task(s)");
        }
        for joined in folia_core::join::take_pending_joins() {
            let name = JObject::from(env.new_string(joined.name)?);
            let uuid = JObject::from(env.new_string(joined.uuid)?);
            call(
                &mut env,
                &bridge,
                "firePlayerJoin",
                "(Ljava/lang/String;Ljava/lang/String;)V",
                &[JValue::Object(&name), JValue::Object(&uuid)],
            )
            .context("PlayerJoinEvent dispatch failed")?;
        }
        tick_tasks(&mut env);
        call(&mut env, &bridge, "tick", "()V", &[])?;
        folia_core::entity::tick_all();
        ticks += 1;
        if max_ticks.is_some_and(|limit| ticks >= limit) {
            break;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    println!("[rust] server loop stopped after {ticks} ticks");

    call(&mut env, &bridge, "shutdown", "()V", &[]).context("plugin host shutdown failed")?;
    // Drop plugin task references while the JVM is still alive.
    TASKS
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .clear();
    println!("[rust] shutdown complete");
    Ok(())
}

fn call(env: &mut JNIEnv, class: &JClass, name: &str, sig: &str, args: &[JValue]) -> Result<()> {
    if let Err(error) = env.call_static_method(class, name, sig, args) {
        let _ = env.exception_describe();
        let _ = env.exception_clear();
        return Err(error.into());
    }
    Ok(())
}

fn build_classpath(root: &Path) -> Result<String> {
    let mut entries = vec![root.join("java").join("bridge").join("classes")];
    let local_libraries = root.join("libraries");
    let libraries = if local_libraries.is_dir() {
        local_libraries
    } else {
        root.join("..").join("libraries")
    };
    collect_jars(&libraries, &mut entries)?;
    let separator = if cfg!(windows) { ';' } else { ':' };
    Ok(entries
        .iter()
        .map(|entry| entry.to_string_lossy().into_owned())
        .collect::<Vec<_>>()
        .join(&separator.to_string()))
}

fn collect_jars(dir: &Path, out: &mut Vec<PathBuf>) -> Result<()> {
    if !dir.exists() {
        return Ok(());
    }
    for entry in std::fs::read_dir(dir)? {
        let path = entry?.path();
        if path.is_dir() {
            collect_jars(&path, out)?;
        } else if path.extension().is_some_and(|ext| ext == "jar") {
            out.push(path);
        }
    }
    Ok(())
}

fn ensure_java_home() {
    if std::env::var_os("JAVA_HOME").is_some() {
        return;
    }

    let candidates = [
        r"C:\Program Files\Java\jdk-21",
        r"C:\Program Files\Eclipse Adoptium\jdk-21",
        r"C:\Program Files\Microsoft\jdk-21",
    ];

    for candidate in candidates {
        let home = Path::new(candidate);
        let windows_jvm = home.join("bin").join("server").join("jvm.dll");
        let unix_jvm = home.join("lib").join("server").join("libjvm.so");
        if windows_jvm.exists() || unix_jvm.exists() {
            // SAFETY: this runs before any JVM threads are started.
            unsafe { std::env::set_var("JAVA_HOME", home) };
            return;
        }
    }
}
