use crate::ast::*;
use crate::code_gen::CodeGen;
use crate::error::Error;
use crate::hir::class_dict::ClassDict;
use crate::hir::hir_maker_context::*;
use crate::hir::method_dict::MethodDict;
use crate::hir::*;
use crate::library::LibraryExports;
use crate::names;
use crate::type_checking;
use ctx_stack::CtxStack;

#[derive(Debug)]
pub struct HirMaker<'hir_maker> {
    /// List of classes found so far
    pub(super) class_dict: ClassDict<'hir_maker>,
    /// List of methods found so far
    pub(super) method_dict: MethodDict,
    /// List of constants found so far
    pub(super) constants: HashMap<ConstFullname, TermTy>,
    /// Constants defined from other library
    pub(super) imported_constants: &'hir_maker HashMap<ConstFullname, TermTy>,
    /// Expressions that initialize constants
    pub(super) const_inits: Vec<HirExpression>,
    /// List of string literals found so far
    pub(super) str_literals: Vec<String>,
    /// Contextual information
    pub(super) ctx_stack: CtxStack,
    /// Counter to give unique name for lambdas
    pub(super) lambda_ct: usize,
    /// Counter for unique name
    pub(super) gensym_ct: usize,
}

pub fn make_hir(
    ast: ast::Program,
    corelib: Option<Corelib>,
    imports: &LibraryExports,
) -> Result<Hir, Error> {
    let (core_classes, core_methods) = if let Some(c) = corelib {
        (c.sk_classes, c.sk_methods)
    } else {
        (Default::default(), Default::default())
    };
    let class_dict = class_dict::create(&ast, core_classes, &imports.sk_classes)?;
    let mut hir_maker = HirMaker::new(class_dict, &imports.constants);
    hir_maker.define_class_constants();
    let (main_exprs, main_lvars) = hir_maker.convert_toplevel_items(&ast.toplevel_items)?;
    let mut hir = hir_maker.extract_hir(main_exprs, main_lvars);

    // While corelib classes are included in `class_dict`,
    // corelib methods are not. Here we need to add them manually
    hir.add_methods(core_methods);

    Ok(hir)
}

impl<'hir_maker> HirMaker<'hir_maker> {
    fn new(
        class_dict: ClassDict<'hir_maker>,
        imported_constants: &'hir_maker HashMap<ConstFullname, TermTy>,
    ) -> HirMaker<'hir_maker> {
        HirMaker {
            class_dict,
            method_dict: MethodDict::new(),
            constants: HashMap::new(),
            imported_constants,
            const_inits: vec![],
            str_literals: vec![],
            ctx_stack: CtxStack::new(vec![HirMakerContext::toplevel()]),
            lambda_ct: 0,
            gensym_ct: 0,
        }
    }

    /// Destructively convert self to Hir
    fn extract_hir(&mut self, main_exprs: HirExpressions, main_lvars: HirLVars) -> Hir {
        // Extract data from self
        let sk_classes = std::mem::take(&mut self.class_dict.sk_classes);
        let sk_methods = std::mem::take(&mut self.method_dict.sk_methods);
        let mut constants = HashMap::new();
        std::mem::swap(&mut constants, &mut self.constants);
        let mut str_literals = vec![];
        std::mem::swap(&mut str_literals, &mut self.str_literals);
        let mut const_inits = vec![];
        std::mem::swap(&mut const_inits, &mut self.const_inits);

        Hir {
            sk_classes,
            sk_methods,
            constants,
            str_literals,
            const_inits,
            main_exprs,
            main_lvars,
        }
    }

    /// Register constants which hold class object
    /// eg.
    /// - ::Int
    /// - ::Meta:Int
    fn define_class_constants(&mut self) {
        for (name, const_is_obj) in self.class_dict.constant_list() {
            let resolved = ResolvedConstName::unsafe_create(name);
            if const_is_obj {
                // Create constant like `Void`, `Maybe::None`.
                let str_idx = self.register_string_literal(&resolved.string());
                let ty = ty::raw(&resolved.string());
                // The class
                let cls_obj =
                    Hir::class_literal(ty.meta_ty(), resolved.to_class_fullname(), str_idx);
                // The instance
                let expr = Hir::method_call(
                    ty,
                    cls_obj,
                    method_fullname(&metaclass_fullname(&resolved.string()), "new"),
                    vec![],
                );
                self.register_const_full(resolved.to_const_fullname(), expr);
            } else {
                let ty = ty::meta(&resolved.string());
                let str_idx = self.register_string_literal(&resolved.string());
                let expr = Hir::class_literal(ty.clone(), resolved.to_class_fullname(), str_idx);
                self.register_const_full(resolved.to_const_fullname(), expr);
            }
        }
    }

    fn convert_toplevel_items(
        &mut self,
        items: &[ast::TopLevelItem],
    ) -> Result<(HirExpressions, HirLVars), Error> {
        let mut main_exprs = vec![];
        for item in items {
            match item {
                ast::TopLevelItem::Def(def) => {
                    self.process_toplevel_def(def)?;
                }
                ast::TopLevelItem::Expr(expr) => {
                    main_exprs.push(self.convert_expr(expr)?);
                }
            }
        }
        debug_assert!(self.ctx_stack.len() == 1);
        let mut toplevel_ctx = self.ctx_stack.pop_toplevel_ctx();
        Ok((
            HirExpressions::new(main_exprs),
            extract_lvars(&mut toplevel_ctx.lvars),
        ))
    }

    fn process_toplevel_def(&mut self, def: &ast::Definition) -> Result<(), Error> {
        let namespace = Namespace::root();
        match def {
            // Extract instance/class methods
            ast::Definition::ClassDefinition {
                name,
                typarams,
                defs,
                ..
            } => {
                self.process_class_def(&namespace, name, typarams.clone(), defs)?;
            }
            ast::Definition::EnumDefinition {
                name,
                typarams,
                cases,
            } => self.process_enum_def(&namespace, name, typarams.clone(), cases)?,
            ast::Definition::ConstDefinition { name, expr } => {
                self.register_toplevel_const(name, expr)?;
            }
            _ => panic!("should be checked in hir::class_dict"),
        }
        Ok(())
    }

    /// Process a class definition and its inner defs
    fn process_class_def(
        &mut self,
        namespace: &Namespace,
        firstname: &ClassFirstname,
        typarams: Vec<TyParam>,
        defs: &[ast::Definition],
    ) -> Result<(), Error> {
        let fullname = namespace.class_fullname(firstname);
        let meta_name = fullname.meta_name();
        self.ctx_stack
            .push(HirMakerContext::class(namespace.add(firstname), typarams));

        // Register constants before processing #initialize
        let inner_namespace = namespace.add(firstname);
        self._process_const_defs_in_class(&inner_namespace, defs)?;

        // Register #initialize and ivars
        let own_ivars =
            self._process_initialize(&fullname, defs.iter().find(|d| d.is_initializer()))?;
        if !own_ivars.is_empty() {
            // Be careful not to reset ivars of corelib/* by builtin/*
            self.class_dict.define_ivars(&fullname, own_ivars.clone());
            self.define_accessors(&fullname, own_ivars, defs);
        }

        // Register .new
        if fullname.0 != "Class" {
            self.method_dict
                .add_method(&meta_name, self.create_new(&fullname, false)?);
        }

        // Process inner defs
        for def in defs {
            match def {
                ast::Definition::InstanceMethodDefinition {
                    sig, body_exprs, ..
                } => {
                    if def.is_initializer() {
                        // Already processed above
                    } else {
                        log::trace!("method {}#{}", &fullname, &sig.name);
                        let method = self.convert_method_def(&fullname, &sig.name, body_exprs)?;
                        self.method_dict.add_method(&fullname, method);
                    }
                }
                ast::Definition::ClassMethodDefinition {
                    sig, body_exprs, ..
                } => {
                    log::trace!("method {}.{}", &fullname, &sig.name);
                    let method = self.convert_method_def(&meta_name, &sig.name, body_exprs)?;
                    self.method_dict.add_method(&meta_name, method);
                }
                ast::Definition::ConstDefinition { .. } => {
                    // Already processed above
                }
                ast::Definition::ClassDefinition {
                    name,
                    defs,
                    typarams,
                    ..
                } => self.process_class_def(&inner_namespace, name, typarams.clone(), defs)?,
                ast::Definition::EnumDefinition {
                    name,
                    typarams,
                    cases,
                } => self.process_enum_def(&inner_namespace, name, typarams.clone(), cases)?,
            }
        }
        self.ctx_stack.pop();
        Ok(())
    }

    /// Add `#initialize` and return defined ivars
    fn _process_initialize(
        &mut self,
        fullname: &ClassFullname,
        initialize: Option<&ast::Definition>,
    ) -> Result<SkIVars, Error> {
        let mut own_ivars = HashMap::default();
        if let Some(ast::Definition::InstanceMethodDefinition {
            sig, body_exprs, ..
        }) = initialize
        {
            log::trace!("method {}#initialize", &fullname);
            let (sk_method, found_ivars) =
                self.create_initialize(fullname, &sig.name, body_exprs)?;
            self.method_dict.add_method(fullname, sk_method);
            own_ivars = found_ivars;
        }
        Ok(own_ivars)
    }

    /// Register constants defined in a class
    fn _process_const_defs_in_class(
        &mut self,
        namespace: &Namespace,
        defs: &[ast::Definition],
    ) -> Result<(), Error> {
        for def in defs {
            if let ast::Definition::ConstDefinition { name, expr } = def {
                let full = namespace.const_fullname(name);
                let hir_expr = self.convert_expr(expr)?;
                self.register_const_full(full, hir_expr);
            }
        }
        Ok(())
    }

    /// Create the `initialize` method
    /// Also, define ivars
    fn create_initialize(
        &mut self,
        class_fullname: &ClassFullname,
        name: &MethodFirstname,
        body_exprs: &[AstExpression],
    ) -> Result<(SkMethod, SkIVars), Error> {
        let super_ivars = self.class_dict.superclass_ivars(class_fullname);
        self.convert_method_def_(class_fullname, name, body_exprs, super_ivars)
    }

    /// Create .new
    fn create_new(
        &self,
        class_fullname: &ClassFullname,
        const_is_obj: bool,
    ) -> Result<SkMethod, Error> {
        let (initialize_name, init_cls_name) =
            self._find_initialize(&class_fullname.instance_ty())?;
        let (signature, _) = self.class_dict.lookup_method(
            &class_fullname.class_ty(),
            &method_firstname("new"),
            Default::default(),
        )?;
        let arity = signature.params.len();
        let classname = class_fullname.clone();

        let new_body = move |code_gen: &CodeGen, function: &inkwell::values::FunctionValue| {
            code_gen.gen_body_of_new(
                function.get_params(),
                &classname,
                &initialize_name,
                &init_cls_name,
                arity,
                const_is_obj,
            );
            Ok(())
        };

        Ok(SkMethod {
            signature,
            body: SkMethodBody::RustClosureMethodBody {
                boxed_gen: Box::new(new_body),
            },
            lvars: vec![],
        })
    }

    /// Find actual `initialize` func to call from `.new`
    fn _find_initialize(&self, class: &TermTy) -> Result<(MethodFullname, ClassFullname), Error> {
        let (_, found_cls) = self.class_dict.lookup_method(
            class,
            &method_firstname("initialize"),
            Default::default(),
        )?;
        Ok((names::method_fullname(&found_cls, "initialize"), found_cls))
    }

    /// Register a constant defined in the toplevel
    pub(super) fn register_toplevel_const(
        &mut self,
        name: &str,
        expr: &AstExpression,
    ) -> Result<(), Error> {
        let hir_expr = self.convert_expr(expr)?;
        self.constants
            .insert(toplevel_const(name), hir_expr.ty.clone());
        let op = Hir::const_assign(toplevel_const(name), hir_expr);
        self.const_inits.push(op);
        Ok(())
    }

    /// Register a constant
    pub(super) fn register_const_full(&mut self, fullname: ConstFullname, hir_expr: HirExpression) {
        debug_assert!(!self.constants.contains_key(&fullname));
        self.constants.insert(fullname.clone(), hir_expr.ty.clone());
        let op = Hir::const_assign(fullname, hir_expr);
        self.const_inits.push(op);
    }

    fn convert_method_def(
        &mut self,
        class_fullname: &ClassFullname,
        name: &MethodFirstname,
        body_exprs: &[AstExpression],
    ) -> Result<SkMethod, Error> {
        let (sk_method, _ivars) =
            self.convert_method_def_(class_fullname, name, body_exprs, None)?;
        Ok(sk_method)
    }

    /// Create a SkMethod and return it with ctx.iivars
    fn convert_method_def_(
        &mut self,
        class_fullname: &ClassFullname,
        name: &MethodFirstname,
        body_exprs: &[AstExpression],
        super_ivars: Option<SkIVars>,
    ) -> Result<(SkMethod, HashMap<String, SkIVar>), Error> {
        // MethodSignature is built beforehand by class_dict::new
        let err = format!(
            "[BUG] signature not found ({}/{}/{:?})",
            class_fullname, name, self.class_dict
        );
        let signature = self
            .class_dict
            .find_method(class_fullname, name)
            .expect(&err)
            .clone();

        self.ctx_stack
            .push(HirMakerContext::method(signature.clone(), super_ivars));
        let mut hir_exprs = self.convert_exprs(body_exprs)?;
        // Insert ::Void so that last expr always matches to ret_ty
        if signature.ret_ty.is_void_type() {
            hir_exprs.voidify();
        }
        let mut method_ctx = self.ctx_stack.pop_method_ctx();
        let lvars = extract_lvars(&mut method_ctx.lvars);
        type_checking::check_return_value(&self.class_dict, &signature, &hir_exprs.ty)?;

        let body = SkMethodBody::ShiikaMethodBody { exprs: hir_exprs };
        Ok((
            SkMethod {
                signature,
                body,
                lvars,
            },
            method_ctx.iivars,
        ))
    }

    /// Process a enum definition
    fn process_enum_def(
        &mut self,
        namespace: &Namespace,
        firstname: &ClassFirstname,
        _typarams: Vec<TyParam>,
        cases: &[ast::EnumCase],
    ) -> Result<(), Error> {
        let inner_namespace = namespace.add(firstname);
        for case in cases {
            self._register_enum_case_class(&inner_namespace, case)?;
        }
        Ok(())
    }

    /// Create a enum case class
    fn _register_enum_case_class(
        &mut self,
        namespace: &Namespace,
        case: &EnumCase,
    ) -> Result<(), Error> {
        let fullname = namespace.class_fullname(&case.name);

        // Register #initialize
        let signature = self
            .class_dict
            .find_method(&fullname, &method_firstname("initialize"))
            .unwrap();
        let self_ty = ty::raw(&fullname.0);
        let exprs = signature
            .params
            .iter()
            .enumerate()
            .map(|(idx, param)| {
                let argref = Hir::arg_ref(param.ty.clone(), idx);
                Hir::ivar_assign(&param.name, idx, argref, false, self_ty.clone())
            })
            .collect();
        let body = SkMethodBody::ShiikaMethodBody {
            exprs: HirExpressions::new(exprs),
        };
        let initialize = SkMethod {
            signature: signature.clone(),
            body,
            lvars: Default::default(),
        };
        self.method_dict.add_method(&fullname, initialize);

        // Register accessors
        let ivars = self.class_dict.get_class(&fullname).ivars.clone();
        self.define_accessors(&fullname, ivars, Default::default());

        // Register .new
        let const_is_obj = case.params.is_empty();
        self.method_dict.add_method(
            &fullname.meta_name(),
            self.create_new(&fullname, const_is_obj)?,
        );
        Ok(())
    }

    /// Generate special lvar name
    pub fn generate_lvar_name(&mut self, prefix: &str) -> String {
        let n = self.gensym_ct;
        self.gensym_ct += 1;
        // Suffix `_` because llvm may add numbers after this name
        // eg.
        //   %"expr@0_3" = load %Maybe*, %Maybe** %"expr@0"
        format!("{}@{}_", prefix, n)
    }
}

/// Destructively extract list of local variables
pub fn extract_lvars(lvars: &mut HashMap<String, CtxLVar>) -> HirLVars {
    std::mem::take(lvars)
        .into_iter()
        .map(|(name, ctx_lvar)| (name, ctx_lvar.ty))
        .collect::<Vec<_>>()
}
