use std::collections::HashMap;
use syn::{
    Expr, File, ItemUse, Macro, Path, UseTree,
    visit_mut::{self, VisitMut},
};

/// 依赖解析器：建立标识符到完整路径的映射
struct DependencyResolver {
    /// 映射表：Ident -> FullPath (e.g., "Local" -> "chrono::Local")
    pub mappings: HashMap<String, String>,
}

impl DependencyResolver {
    fn new() -> Self {
        Self {
            mappings: HashMap::new(),
        }
    }
}

impl<'ast> VisitMut for DependencyResolver {
    fn visit_item_use_mut(&mut self, i: &mut ItemUse) {
        // 处理 use crate::Item; 这种形式
        if let UseTree::Path(path) = &i.tree {
            if let UseTree::Name(name) = &*path.tree {
                let crate_name = path.ident.to_string();
                let item_name = name.ident.to_string();
                // 忽略自引用或标准库
                if crate_name != "crate" && crate_name != "std" && crate_name != "core" {
                    let full_path = format!("{}::{}", crate_name, item_name);
                    self.mappings.insert(item_name, full_path);
                }
            }
        }

        // 递归处理嵌套的 UseTree (比如 use a::{b, c})
        visit_mut::visit_item_use_mut(self, i);
    }
}

/// 路径重写器：将 op! 中的简短路径替换为完整路径
struct PathRewriter {
    pub mappings: HashMap<String, String>,
}

impl PathRewriter {
    fn rewrite_macro(mac: &mut Macro, mappings: &HashMap<String, String>) {
        // 尝试解析宏内部的 TokenStream 为表达式
        if let Ok(mut expr) = syn::parse2::<Expr>(mac.tokens.clone()) {
            let mut rewriter = PathRewriter {
                mappings: mappings.clone(),
            };
            visit_mut::visit_expr_mut(&mut rewriter, &mut expr);
            mac.tokens = quote::quote!(#expr);
        }
    }
}

impl VisitMut for PathRewriter {
    fn visit_expr_mut(&mut self, expr: &mut Expr) {
        match expr {
            Expr::Macro(expr_mac) => {
                // 检查是否是 op! 宏 (支持 preprocessor::op 和 op)
                let is_op = expr_mac
                    .mac
                    .path
                    .segments
                    .last()
                    .map_or(false, |s| s.ident == "op");

                if is_op {
                    Self::rewrite_macro(&mut expr_mac.mac, &self.mappings);
                } else {
                    // 递归处理其他宏
                    visit_mut::visit_expr_mut(self, expr);
                }
            }
            _ => {
                // 递归处理子表达式
                visit_mut::visit_expr_mut(self, expr);

                // 处理当前节点的路径
                if let Expr::Path(path_expr) = expr {
                    // 检查路径的第一个分段是否在映射表中
                    if !path_expr.path.segments.is_empty() && path_expr.qself.is_none() {
                        let first_ident = &path_expr.path.segments[0].ident;
                        let name = first_ident.to_string();

                        // 只有当路径只有一个分段时才进行替换，避免破坏 Local::now() 这种调用
                        if path_expr.path.segments.len() == 1 {
                            if let Some(full_path_str) = self.mappings.get(&name) {
                                if let Ok(new_path) = syn::parse_str::<Path>(full_path_str) {
                                    path_expr.path = new_path;
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}

pub fn process_file(mut file: File) -> File {
    // 1. 扫描 use 语句建立映射
    let mut resolver = DependencyResolver::new();
    resolver.visit_file_mut(&mut file);

    // 2. 遍历文件，重写 op! 宏调用
    let mut rewriter = PathRewriter {
        mappings: resolver.mappings,
    };

    rewriter.visit_file_mut(&mut file);

    file
}
