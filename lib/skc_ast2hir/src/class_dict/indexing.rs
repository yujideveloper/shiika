use crate::class_dict::*;
use crate::error;
use crate::parse_typarams;
use anyhow::Result;
use shiika_ast;
use shiika_core::{names::*, ty, ty::*};
use skc_hir::signature::*;
use skc_hir::*;
use std::collections::HashMap;

type MethodSignatures = HashMap<MethodFirstname, MethodSignature>;

impl<'hir_maker> ClassDict<'hir_maker> {
    /// Register a class or module
    pub fn add_type(&mut self, sk_type_: impl Into<SkType>) {
        let sk_type = sk_type_.into();
        self.sk_types.insert(sk_type.base().fullname_(), sk_type);
    }

    /// Add a method
    /// Used to add auto-defined accessors
    pub fn add_method(&mut self, clsname: &ClassFullname, sig: MethodSignature) {
        let sk_class = self.sk_types.get_mut(clsname).unwrap();
        sk_class
            .base_mut()
            .method_sigs
            .insert(sig.fullname.first_name.clone(), sig);
    }

    pub fn index_program(&mut self, toplevel_defs: &[&shiika_ast::Definition]) -> Result<()> {
        let namespace = Namespace::root();
        for def in toplevel_defs {
            match def {
                shiika_ast::Definition::ClassDefinition {
                    name,
                    typarams,
                    supers,
                    defs,
                } => {
                    self.index_class(&namespace, name, parse_typarams(typarams), supers, defs)?
                }
                shiika_ast::Definition::ModuleDefinition {
                    name,
                    typarams,
                    defs,
                } => self.index_module(&namespace, name, parse_typarams(typarams), defs)?,
                shiika_ast::Definition::EnumDefinition {
                    name,
                    typarams,
                    cases,
                    defs,
                } => self.index_enum(&namespace, name, parse_typarams(typarams), cases, defs)?,
                shiika_ast::Definition::ConstDefinition { .. } => (),
                _ => {
                    return Err(error::syntax_error(&format!(
                        "must not be toplevel: {:?}",
                        def
                    )))
                }
            }
        }
        Ok(())
    }

    fn index_class(
        &mut self,
        namespace: &Namespace,
        firstname: &ClassFirstname,
        typarams: Vec<ty::TyParam>,
        supers: &[UnresolvedTypeName],
        defs: &[shiika_ast::Definition],
    ) -> Result<()> {
        let fullname = namespace.class_fullname(firstname);
        let metaclass_fullname = fullname.meta_name();
        let (superclass, includes) = self._resolve_supers(namespace, &typarams, supers)?;
        let new_sig = if fullname.0 == "Never" {
            None
        } else {
            Some(signature::signature_of_new(
                &metaclass_fullname,
                self._initializer_params(namespace, &typarams, &superclass, defs)?,
                &ty::return_type_of_new(&fullname, &typarams),
            ))
        };

        let inner_namespace = namespace.add(firstname);
        let (instance_methods, class_methods) =
            self.index_defs_in_class(&inner_namespace, &fullname, &typarams, defs)?;

        match self.sk_types.get_mut(&fullname) {
            Some(class) => {
                // Merge methods to existing class
                // Shiika will not support reopening a class but this is needed
                // for classes defined both in src corelib/ and in builtin/.
                class.base_mut().method_sigs.extend(instance_methods);
                let metaclass = self
                    .sk_types
                    .get_mut(&metaclass_fullname)
                    .unwrap_or_else(|| {
                        panic!(
                            "[BUG] metaclass not found: {} <- {}",
                            fullname, &metaclass_fullname
                        )
                    });
                metaclass.base_mut().method_sigs.extend(class_methods);
                // Add `.new` to the metaclass
                if let Some(sig) = new_sig {
                    if !metaclass
                        .base()
                        .method_sigs
                        .contains_key(&method_firstname("new"))
                    {
                        metaclass
                            .base_mut()
                            .method_sigs
                            .insert(sig.fullname.first_name.clone(), sig);
                    }
                }
            }
            None => self.add_new_class(
                &fullname,
                &typarams,
                superclass,
                new_sig,
                instance_methods,
                class_methods,
                Some(false),
                false,
            ),
        }
        Ok(())
    }

    /// Resolve superclass and included module names of a class definition 
    fn _resolve_supers(
        &self,
        namespace: &Namespace,
        class_typarams: &[ty::TyParam],
        supers: &[UnresolvedTypeName],
    ) -> Result<(Superclass, Vec<Superclass>)> {
        let mut modules = vec![];
        let mut superclass = None;
        for name in supers {
            let ty = self._resolve_typename(namespace, class_typarams, Default::default(), name)?;
            match self.find_type(&ty.erasure().to_type_fullname()) {
                Some(SkType::Class(c)) => {
                    if !modules.is_empty() {
                        return Err(error::program_error(&format!("superclass must be the first")));
                    }
                    if superclass.is_some() {
                        return Err(error::program_error(&format!("only one superclass is allowed")));
                    }
                    if c.is_final.unwrap() {
                        return Err(error::program_error(&format!("inheriting {} is not allowed", ty)));
                    }
                    superclass = Some(Superclass::from_ty(ty))
                }
                Some(SkType::Module(_)) => {
                    modules.push(Superclass::from_ty(ty));
                }
                None => {
                    return Err(error::program_error(&format!("unknown class or module {}", ty)));
                }
            }
        }
        Ok((superclass.unwrap_or(Superclass::default()),
            modules))
    }

    fn index_module(
        &mut self,
        namespace: &Namespace,
        firstname: &ModuleFirstname,
        typarams: Vec<ty::TyParam>,
        defs: &[shiika_ast::Definition],
    ) -> Result<()> {
        let fullname = namespace.class_fullname(&firstname.to_class_first_name());
        let inner_namespace = namespace.add(&firstname.to_class_first_name());
        let (instance_methods, class_methods, requirements) =
            self.index_defs_in_module(&inner_namespace, &fullname, &typarams, defs)?;

        match self.sk_types.get_mut(&fullname) {
            Some(_) => todo!(),
            None => self.add_new_module(
                &fullname,
                &typarams,
                instance_methods,
                class_methods,
                requirements,
            ),
        }
        Ok(())
    }

    /// Return parameters of `initialize` which is defined by
    /// - `#initialize` in `defs` (if any) or,
    /// - `#initialize` inherited from ancestors.
    fn _initializer_params(
        &self,
        namespace: &Namespace,
        typarams: &[ty::TyParam],
        superclass: &Superclass,
        defs: &[shiika_ast::Definition],
    ) -> Result<Vec<MethodParam>> {
        if let Some(shiika_ast::Definition::InstanceMethodDefinition { sig, .. }) =
            defs.iter().find(|d| d.is_initializer())
        {
            // Has explicit initializer definition
            self.convert_params(namespace, &sig.params, typarams, Default::default())
        } else {
            // Inherit #initialize from superclass
            let (sig, _) = self
                .lookup_method(superclass.ty(), &method_firstname("initialize"), &[])
                .expect("[BUG] initialize not found");
            Ok(specialized_initialize(&sig, superclass).params)
        }
    }

    fn index_enum(
        &mut self,
        namespace: &Namespace,
        firstname: &ClassFirstname,
        typarams: Vec<TyParam>,
        cases: &[shiika_ast::EnumCase],
        defs: &[shiika_ast::Definition],
    ) -> Result<()> {
        let fullname = namespace.class_fullname(firstname);
        let inner_namespace = namespace.add(firstname);
        let (instance_methods, class_methods) =
            self.index_defs_in_class(&inner_namespace, &fullname, &typarams, defs)?;
        self.add_new_class(
            &fullname,
            &typarams,
            Superclass::simple("Object"),
            None,
            instance_methods,
            class_methods,
            Some(true),
            false,
        );
        for case in cases {
            self.index_enum_case(namespace, &fullname, &typarams, case)?;
        }

        Ok(())
    }

    fn index_enum_case(
        &mut self,
        namespace: &Namespace,
        enum_fullname: &ClassFullname,
        typarams: &[ty::TyParam],
        case: &shiika_ast::EnumCase,
    ) -> Result<()> {
        let ivar_list = self._enum_case_ivars(namespace, typarams, case)?;
        let fullname = case.name.add_namespace(&enum_fullname.0);
        let superclass = enum_case_superclass(enum_fullname, typarams, case);
        let (new_sig, initialize_sig) = enum_case_new_sig(&ivar_list, typarams, &fullname);

        let mut instance_methods = enum_case_getters(&fullname, &ivar_list);
        instance_methods.insert(method_firstname("initialize"), initialize_sig);

        let case_typarams = if case.params.is_empty() {
            Default::default()
        } else {
            typarams
        };
        self.add_new_class(
            &fullname,
            case_typarams,
            superclass,
            Some(new_sig),
            instance_methods,
            Default::default(),
            Some(true),
            case.params.is_empty(),
        );
        let ivars = ivar_list.into_iter().map(|x| (x.name.clone(), x)).collect();
        self.define_ivars(&fullname, ivars);
        Ok(())
    }

    /// List up ivars of an enum case
    fn _enum_case_ivars(
        &self,
        namespace: &Namespace,
        typarams: &[ty::TyParam],
        case: &shiika_ast::EnumCase,
    ) -> Result<Vec<SkIVar>> {
        let mut ivars = vec![];
        for (idx, param) in case.params.iter().enumerate() {
            let ty = self._resolve_typename(namespace, typarams, Default::default(), &param.typ)?;
            let ivar = SkIVar {
                idx,
                name: param.name.clone(),
                ty,
                readonly: true,
            };
            ivars.push(ivar);
        }
        Ok(ivars)
    }

    fn index_defs_in_class(
        &mut self,
        namespace: &Namespace,
        fullname: &ClassFullname,
        typarams: &[ty::TyParam],
        defs: &[shiika_ast::Definition],
    ) -> Result<(MethodSignatures, MethodSignatures)> {
        let (instance_methods, class_methods, _) =
            self._index_inner_defs(namespace, fullname, typarams, defs, false)?;
        Ok((instance_methods, class_methods))
    }

    fn index_defs_in_module(
        &mut self,
        namespace: &Namespace,
        fullname: &ClassFullname,
        typarams: &[ty::TyParam],
        defs: &[shiika_ast::Definition],
    ) -> Result<(MethodSignatures, MethodSignatures, Vec<MethodSignature>)> {
        self._index_inner_defs(namespace, fullname, typarams, defs, true)
    }

    fn _index_inner_defs(
        &mut self,
        namespace: &Namespace,
        fullname: &ClassFullname,
        typarams: &[ty::TyParam],
        defs: &[shiika_ast::Definition],
        is_module: bool,
    ) -> Result<(MethodSignatures, MethodSignatures, Vec<MethodSignature>)> {
        let mut instance_methods = HashMap::new();
        let mut class_methods = HashMap::new();
        let mut requirements = vec![];
        for def in defs {
            match def {
                shiika_ast::Definition::InstanceMethodDefinition { sig, .. } => {
                    let hir_sig = self.create_signature(namespace, fullname, sig, typarams)?;
                    instance_methods.insert(sig.name.clone(), hir_sig);
                }
                shiika_ast::Definition::ClassMethodDefinition { sig, .. } => {
                    let hir_sig = self.create_signature(
                        namespace,
                        &fullname.meta_name(),
                        sig,
                        Default::default(),
                    )?;
                    class_methods.insert(sig.name.clone(), hir_sig);
                }
                shiika_ast::Definition::ConstDefinition { .. } => (),
                shiika_ast::Definition::ClassDefinition {
                    name,
                    typarams,
                    supers,
                    defs,
                } => {
                    self.index_class(namespace, name, parse_typarams(typarams), supers, defs)?;
                }
                shiika_ast::Definition::ModuleDefinition {
                    name,
                    typarams,
                    defs,
                } => {
                    self.index_module(namespace, name, parse_typarams(typarams), defs)?;
                }
                shiika_ast::Definition::MethodRequirementDefinition { sig } => {
                    if is_module {
                        let hir_sig = self.create_signature(namespace, fullname, sig, typarams)?;
                        requirements.push(hir_sig);
                    } else {
                        return Err(error::syntax_error(&format!(
                            "only modules have method requirement: {:?} {:?} {:?}",
                            namespace, fullname, sig
                        )));
                    }
                }
                shiika_ast::Definition::EnumDefinition {
                    name,
                    typarams,
                    cases,
                    defs,
                } => {
                    self.index_enum(namespace, name, parse_typarams(typarams), cases, defs)?;
                }
            }
        }
        Ok((instance_methods, class_methods, requirements))
    }

    /// Register a class and its metaclass to self
    // REFACTOR: fix too_many_arguments
    #[allow(clippy::too_many_arguments)]
    fn add_new_class(
        &mut self,
        fullname: &ClassFullname,
        typarams: &[ty::TyParam],
        superclass: Superclass,
        new_sig: Option<MethodSignature>,
        instance_methods: HashMap<MethodFirstname, MethodSignature>,
        mut class_methods: HashMap<MethodFirstname, MethodSignature>,
        is_final: Option<bool>,
        const_is_obj: bool,
    ) {
        // Add `.new` to the metaclass
        if let Some(sig) = new_sig {
            class_methods.insert(sig.fullname.first_name.clone(), sig);
        }

        let base = SkTypeBase {
            erasure: Erasure::nonmeta(&fullname.0),
            typarams: typarams.to_vec(),
            method_sigs: instance_methods,
            foreign: false,
        };
        self.add_type(SkClass {
            base,
            superclass: Some(superclass),
            ivars: HashMap::new(), // will be set when processing `#initialize`
            is_final,
            const_is_obj,
        });

        // Create metaclass (which is a subclass of `Class`)
        let the_class = self.get_class(&class_fullname("Class"));
        let meta_ivars = the_class.ivars.clone();
        let base = SkTypeBase {
            erasure: Erasure::meta(&fullname.0),
            typarams: typarams.to_vec(),
            method_sigs: class_methods,
            foreign: false,
        };
        self.add_type(SkClass {
            base,
            superclass: Some(Superclass::simple("Class")),
            ivars: meta_ivars,
            is_final: None,
            const_is_obj: false,
        });
    }

    /// Register a class and its metaclass to self
    fn add_new_module(
        &mut self,
        fullname: &ClassFullname,
        typarams: &[ty::TyParam],
        instance_methods: HashMap<MethodFirstname, MethodSignature>,
        class_methods: HashMap<MethodFirstname, MethodSignature>,
        requirements: Vec<MethodSignature>,
    ) {
        let base = SkTypeBase {
            erasure: Erasure::nonmeta(&fullname.0),
            typarams: typarams.to_vec(),
            method_sigs: instance_methods,
            foreign: false,
        };
        self.add_type(SkModule { base, requirements });

        // Create metaclass (which is a subclass of `Class`)
        let the_class = self.get_class(&class_fullname("Class"));
        let meta_ivars = the_class.ivars.clone();
        let base = SkTypeBase {
            erasure: Erasure::meta(&fullname.0),
            typarams: typarams.to_vec(),
            method_sigs: class_methods,
            foreign: false,
        };
        self.add_type(SkClass {
            base,
            superclass: Some(Superclass::simple("Class")),
            ivars: meta_ivars,
            is_final: None,
            const_is_obj: false,
        });
    }

    /// Convert AstMethodSignature to MethodSignature
    pub fn create_signature(
        &self,
        namespace: &Namespace,
        class_fullname: &ClassFullname,
        sig: &shiika_ast::AstMethodSignature,
        class_typarams: &[ty::TyParam],
    ) -> Result<MethodSignature> {
        let method_typarams = parse_typarams(&sig.typarams);
        let fullname = method_fullname(class_fullname, &sig.name.0);
        let ret_ty = if let Some(typ) = &sig.ret_typ {
            self._resolve_typename(namespace, class_typarams, &method_typarams, typ)?
        } else {
            ty::raw("Void") // Default return type.
        };
        Ok(MethodSignature {
            fullname,
            ret_ty,
            params: self.convert_params(
                namespace,
                &sig.params,
                class_typarams,
                &method_typarams,
            )?,
            typarams: method_typarams,
        })
    }

    /// Convert ast params to hir params
    pub fn convert_params(
        &self,
        namespace: &Namespace,
        ast_params: &[shiika_ast::Param],
        class_typarams: &[ty::TyParam],
        method_typarams: &[ty::TyParam],
    ) -> Result<Vec<MethodParam>> {
        let mut hir_params = vec![];
        for param in ast_params {
            hir_params.push(MethodParam {
                name: param.name.to_string(),
                ty: self._resolve_typename(
                    namespace,
                    class_typarams,
                    method_typarams,
                    &param.typ,
                )?,
            });
        }
        Ok(hir_params)
    }

    /// Resolve the given type name to fullname
    fn _resolve_typename(
        &self,
        namespace: &Namespace,
        class_typarams: &[ty::TyParam],
        method_typarams: &[ty::TyParam],
        name: &UnresolvedTypeName,
    ) -> Result<TermTy> {
        // Check it is a typaram
        if name.args.is_empty() && name.names.len() == 1 {
            let s = name.names.first().unwrap();
            if let Some(idx) = class_typarams.iter().position(|t| *s == t.name) {
                return Ok(ty::typaram_ref(s, TyParamKind::Class, idx).into_term_ty());
            } else if let Some(idx) = method_typarams.iter().position(|t| *s == t.name) {
                return Ok(ty::typaram_ref(s, TyParamKind::Method, idx).into_term_ty());
            }
        }
        // Otherwise:
        let mut tyargs = vec![];
        for arg in &name.args {
            tyargs.push(self._resolve_typename(namespace, class_typarams, method_typarams, arg)?);
        }
        let (resolved_base, base_typarams) =
            self._resolve_simple_typename(namespace, &name.names)?;
        if name.args.len() != base_typarams.len() {
            return Err(error::type_error(&format!(
                "wrong number of type arguments: {:?}",
                name
            )));
        }
        Ok(ty::nonmeta(&resolved_base, tyargs))
    }

    /// Resolve the given type name (without type arguments) to fullname
    /// Also returns the typarams of the class, if any
    fn _resolve_simple_typename(
        &self,
        namespace: &Namespace,
        names: &[String],
    ) -> Result<(Vec<String>, &[TyParam])> {
        let n = namespace.size();
        for k in 0..=n {
            let mut resolved = namespace.head(n - k).to_vec();
            resolved.append(&mut names.to_vec());
            if let Some(typarams) = self.class_index.get(&class_fullname(resolved.join("::"))) {
                return Ok((resolved, typarams));
            }
        }
        Err(error::name_error(&format!(
            "unknown type {:?} in {:?}",
            names, namespace,
        )))
    }
}

/// Returns superclass of a enum case
fn enum_case_superclass(
    enum_fullname: &ClassFullname,
    typarams: &[ty::TyParam],
    case: &shiika_ast::EnumCase,
) -> Superclass {
    if case.params.is_empty() {
        // eg. Maybe::None : Maybe<Never>
        let tyargs = typarams
            .iter()
            .map(|_| ty::raw("Never"))
            .collect::<Vec<_>>();
        Superclass::new(enum_fullname, tyargs)
    } else {
        // eg. Maybe::Some<out V> : Maybe<V>
        let tyargs = typarams
            .iter()
            .enumerate()
            .map(|(i, t)| ty::typaram_ref(&t.name, TyParamKind::Class, i).into_term_ty())
            .collect::<Vec<_>>();
        Superclass::new(enum_fullname, tyargs)
    }
}

/// Returns signature of `.new` and `#initialize` of an enum case
fn enum_case_new_sig(
    ivar_list: &[SkIVar],
    typarams: &[ty::TyParam],
    fullname: &ClassFullname,
) -> (MethodSignature, MethodSignature) {
    let params = ivar_list
        .iter()
        .map(|ivar| MethodParam {
            name: ivar.name.to_string(),
            ty: ivar.ty.clone(),
        })
        .collect::<Vec<_>>();
    let ret_ty = if ivar_list.is_empty() {
        ty::raw(&fullname.0)
    } else {
        let tyargs = typarams
            .iter()
            .enumerate()
            .map(|(i, t)| ty::typaram_ref(&t.name, TyParamKind::Class, i).into_term_ty())
            .collect::<Vec<_>>();
        ty::spe(&fullname.0, tyargs)
    };
    (
        signature::signature_of_new(&fullname.meta_name(), params.clone(), &ret_ty),
        signature::signature_of_initialize(fullname, params),
    )
}

/// Create signatures of getters of an enum case
fn enum_case_getters(case_fullname: &ClassFullname, ivars: &[SkIVar]) -> MethodSignatures {
    ivars
        .iter()
        .map(|ivar| {
            let sig = MethodSignature {
                fullname: method_fullname(case_fullname, &ivar.accessor_name()),
                ret_ty: ivar.ty.clone(),
                params: Default::default(),
                typarams: Default::default(),
            };
            (method_firstname(&ivar.name), sig)
        })
        .collect()
}
