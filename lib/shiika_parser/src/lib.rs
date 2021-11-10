#![feature(backtrace)]

/// Parser
///
/// Implementaion rules
/// - Call `skip_ws`/`skip_wsn` before calling other `parse_xx`

macro_rules! parse_error {
    ( $self:ident, $( $arg:expr ),* ) => ({
        let rest = $self.lexer.peek_n(100);
        let msg = format!( $( $arg ),* ) + " | " + &rest;
        $self.parseerror(&msg)
    })
}

mod base;
mod definition_parser;
mod error;
mod expression_parser;
pub mod lexer;
use crate::error::Error;
use crate::lexer::Lexer;
use crate::lexer::LexerState;
use shiika_ast as ast;
use shiika_ast::Token;

pub struct Parser<'a> {
    pub lexer: Lexer<'a>,
    /// For debug print
    pub lv: usize,
}

impl<'a> Parser<'a> {
    pub fn new(src: &str) -> Parser {
        Parser {
            lexer: Lexer::new(src),
            lv: 0,
        }
    }

    /// Parse a method signature
    pub fn parse_signature(sig_str: &str) -> Result<ast::AstMethodSignature, Error> {
        let mut parser = Parser {
            lexer: Lexer::new_with_state(sig_str, LexerState::MethodName),
            lv: 0,
        };
        let (ast_sig, _) = parser.parse_method_signature()?;
        Ok(ast_sig)
    }

    pub fn parse(src: &str) -> Result<ast::Program, Error> {
        let mut parser = Parser::new(src);
        parser.parse_program()
    }

    fn parse_program(&mut self) -> Result<ast::Program, Error> {
        self.skip_wsn()?;
        let toplevel_items = self.parse_toplevel_items()?;
        self.expect_eof()?;
        Ok(ast::Program { toplevel_items })
    }

    pub fn expect_eof(&self) -> Result<(), Error> {
        if *self.current_token() != Token::Eof {
            return Err(parse_error!(
                self,
                "unexpected token: {:?}",
                self.current_token()
            ));
        }
        Ok(())
    }

    fn parse_toplevel_items(&mut self) -> Result<Vec<ast::TopLevelItem>, Error> {
        let mut items = vec![];
        loop {
            match self.current_token() {
                Token::KwClass => {
                    items.push(ast::TopLevelItem::Def(self.parse_class_definition()?));
                }
                Token::KwEnum => {
                    items.push(ast::TopLevelItem::Def(self.parse_enum_definition()?));
                }
                Token::KwDef => {
                    return Err(parse_error!(
                        self,
                        "you cannot define toplevel method in Shiika"
                    ));
                }
                Token::Eof | Token::KwEnd => break,
                _ => {
                    let expr = self.parse_expr()?;
                    if let Some(constdef) = expr.as_const_def() {
                        items.push(ast::TopLevelItem::Def(constdef));
                    } else {
                        items.push(ast::TopLevelItem::Expr(expr));
                    }
                }
            }
            self.skip_wsn()?;
        }
        Ok(items)
    }
}
