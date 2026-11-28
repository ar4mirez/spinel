//! Cranelift backend: bytecode in, machine code out (phase 6, #120).
//!
//! The interpreter is the reference; the JIT lowers the same [`Iseq`] it runs,
//! instruction by instruction, with no IR of its own in between and no
//! semantics of its own: what an instruction does compiled is what the
//! interpreter's light loops do for it, and anything they would decline is a
//! bail-out back to the interpreter at that instruction. See `docs/engine.md`,
//! "JIT", and [`spinel_vm::jit`] for the boundary.
//!
//! [`lower`] decides what each instruction becomes and [`codegen`] writes it
//! as Cranelift IR. [`install`] gives a heap the backend, which compiles each
//! body the first time a frame of it is entered.

pub mod codegen;
pub mod lower;

use cranelift_codegen::ir::types;
use cranelift_codegen::settings::{self, Configurable};
use cranelift_codegen::{Context, ir};
use cranelift_frontend::FunctionBuilderContext;
use cranelift_jit::{JITBuilder, JITModule};
use cranelift_module::Module;
use spinel_vm::Iseq;
use spinel_vm::heap::HandleScope;
use spinel_vm::jit::{Compiler, Entry};

/// Give `scope`'s heap the JIT, forced on: every body is compiled the first
/// time a frame of it is entered. A new Ractor's heap gets one of its own.
pub fn install(scope: &mut HandleScope<'_>) {
    scope.set_jit(factory);
}

/// The backend for one heap, or `None` when this host has no Cranelift code
/// generator, and the interpreter runs everything.
#[must_use]
pub fn factory() -> Option<Box<dyn Compiler>> {
    Backend::new().map(|backend| Box::new(backend) as Box<dyn Compiler>)
}

/// One heap's code generator and the code it made, which lives as long as
/// it does.
struct Backend {
    /// `None` only while being dropped.
    module: Option<JITModule>,
    context: Context,
    builder: FunctionBuilderContext,
}

// SAFETY: a `JITModule` is not `Send` only because its memory provider is a
// `Box<dyn JITMemoryProvider>` with no `Send` bound; the provider `new`
// installs is the default one, which owns plain mapped regions and nothing
// tied to a thread. The backend is a heap's, and a heap is used by one
// thread at a time.
unsafe impl Send for Backend {}

impl Backend {
    fn new() -> Option<Backend> {
        let mut flags = settings::builder();
        // As `JITBuilder::with_flags` sets them: long-range calls, and code
        // that is not position-independent.
        flags.set("use_colocated_libcalls", "false").ok()?;
        flags.set("is_pic", "false").ok()?;
        flags.set("opt_level", "speed").ok()?;
        let isa = cranelift_native::builder()
            .ok()?
            .finish(settings::Flags::new(flags))
            .ok()?;
        // The words compiled code moves are 64 bits, and so are pointers.
        if isa.pointer_type() != types::I64 {
            return None;
        }
        let builder = JITBuilder::with_isa(isa, cranelift_module::default_libcall_names());
        let module = JITModule::new(builder);
        Some(Backend {
            context: module.make_context(),
            module: Some(module),
            builder: FunctionBuilderContext::new(),
        })
    }
}

impl Compiler for Backend {
    fn compile(&mut self, iseq: &Iseq) -> Option<Entry> {
        let plan = lower::plan(iseq)?;
        let module = self.module.as_mut()?;
        let signature = codegen::signature(module.isa().default_call_conv());
        let id = module.declare_anonymous_function(&signature).ok()?;
        self.context.clear();
        self.context.func =
            ir::Function::with_name_signature(ir::UserFuncName::default(), signature);
        codegen::build(&mut self.context.func, &mut self.builder, &plan);
        let defined = module.define_function(id, &mut self.context);
        module.clear_context(&mut self.context);
        defined.ok()?;
        module.finalize_definitions().ok()?;
        let code = module.get_finalized_function(id);
        // SAFETY: `code` is the function just defined with `signature`, which
        // is `Entry`'s: one pointer-sized argument and a 64-bit answer, in
        // the host's default calling convention, which `extern "C"` is. It
        // lives as long as `module`, which this backend keeps until the heap
        // is dropped, after the last frame that could run it.
        Some(unsafe { std::mem::transmute::<*const u8, Entry>(code) })
    }
}

impl Drop for Backend {
    fn drop(&mut self) {
        if let Some(module) = self.module.take() {
            // SAFETY: the heap that held this backend is being dropped with
            // it, so no frame is running its code and none can call it again.
            unsafe { module.free_memory() };
        }
    }
}
