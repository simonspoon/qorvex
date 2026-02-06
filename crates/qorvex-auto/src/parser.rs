use crate::ast::*;
use crate::error::AutoError;

/// A segment of an interpolated string: either a literal part or a variable reference.
#[derive(Debug, Clone, PartialEq)]
enum StringSegment {
    Literal(String),
    Variable(String),
}

#[derive(Debug, Clone, PartialEq)]
enum Token {
    Ident(String),
    String(String),
    InterpolatedString(Vec<StringSegment>),
    Number(i64),
    LParen,
    RParen,
    LBrace,
    RBrace,
    LBracket,
    RBracket,
    Comma,
    Equals,
    DoubleEquals,
    NotEquals,
    Plus,
    Newline,
    // Keywords
    Foreach,
    In,
    For,
    From,
    To,
    If,
    Else,
}

#[derive(Debug, Clone)]
struct Located {
    token: Token,
    line: usize,
}

fn tokenize(source: &str) -> Result<Vec<Located>, AutoError> {
    let mut tokens = Vec::new();
    let mut chars = source.chars().peekable();
    let mut line = 1usize;

    while let Some(&ch) = chars.peek() {
        match ch {
            '#' => {
                // Skip comment to end of line
                while let Some(&c) = chars.peek() {
                    if c == '\n' {
                        break;
                    }
                    chars.next();
                }
            }
            '\n' => {
                chars.next();
                // Only push Newline if the last token wasn't already a Newline
                if tokens.last().map_or(true, |t: &Located| t.token != Token::Newline) {
                    tokens.push(Located { token: Token::Newline, line });
                }
                line += 1;
            }
            ' ' | '\t' | '\r' => {
                chars.next();
            }
            '(' => { chars.next(); tokens.push(Located { token: Token::LParen, line }); }
            ')' => { chars.next(); tokens.push(Located { token: Token::RParen, line }); }
            '{' => { chars.next(); tokens.push(Located { token: Token::LBrace, line }); }
            '}' => { chars.next(); tokens.push(Located { token: Token::RBrace, line }); }
            '[' => { chars.next(); tokens.push(Located { token: Token::LBracket, line }); }
            ']' => { chars.next(); tokens.push(Located { token: Token::RBracket, line }); }
            ',' => { chars.next(); tokens.push(Located { token: Token::Comma, line }); }
            '+' => { chars.next(); tokens.push(Located { token: Token::Plus, line }); }
            '=' => {
                chars.next();
                if chars.peek() == Some(&'=') {
                    chars.next();
                    tokens.push(Located { token: Token::DoubleEquals, line });
                } else {
                    tokens.push(Located { token: Token::Equals, line });
                }
            }
            '!' => {
                chars.next();
                if chars.peek() == Some(&'=') {
                    chars.next();
                    tokens.push(Located { token: Token::NotEquals, line });
                } else {
                    return Err(AutoError::Parse {
                        message: "Expected '=' after '!'".to_string(),
                        line,
                    });
                }
            }
            '"' | '\'' => {
                let quote = ch;
                let is_double = quote == '"';
                chars.next();
                let mut s = String::new();
                // For double-quoted strings, track segments for interpolation
                let mut segments: Vec<StringSegment> = Vec::new();
                let mut has_interpolation = false;
                loop {
                    match chars.peek() {
                        Some(&'\\') => {
                            chars.next();
                            match chars.next() {
                                Some('n') => s.push('\n'),
                                Some('t') => s.push('\t'),
                                Some('\\') => s.push('\\'),
                                Some('$') if is_double => s.push('$'),
                                Some(c) if c == quote => s.push(c),
                                Some(c) => { s.push('\\'); s.push(c); }
                                None => return Err(AutoError::Parse {
                                    message: "Unterminated string".to_string(),
                                    line,
                                }),
                            }
                        }
                        Some(&'$') if is_double => {
                            chars.next();
                            // Collect identifier after $
                            let mut ident = String::new();
                            while let Some(&c) = chars.peek() {
                                if c.is_ascii_alphanumeric() || c == '_' {
                                    ident.push(c);
                                    chars.next();
                                } else {
                                    break;
                                }
                            }
                            if ident.is_empty() {
                                // Bare $ with no identifier, treat as literal
                                s.push('$');
                            } else {
                                has_interpolation = true;
                                if !s.is_empty() {
                                    segments.push(StringSegment::Literal(std::mem::take(&mut s)));
                                }
                                segments.push(StringSegment::Variable(ident));
                            }
                        }
                        Some(&c) if c == quote => {
                            chars.next();
                            break;
                        }
                        Some(_) => {
                            s.push(chars.next().unwrap());
                        }
                        None => return Err(AutoError::Parse {
                            message: "Unterminated string".to_string(),
                            line,
                        }),
                    }
                }
                if has_interpolation {
                    if !s.is_empty() {
                        segments.push(StringSegment::Literal(s));
                    }
                    tokens.push(Located { token: Token::InterpolatedString(segments), line });
                } else {
                    tokens.push(Located { token: Token::String(s), line });
                }
            }
            c if c.is_ascii_digit() || (c == '-' && chars.clone().nth(1).map_or(false, |n| n.is_ascii_digit())) => {
                let mut num_str = String::new();
                if c == '-' {
                    num_str.push('-');
                    chars.next();
                }
                while let Some(&d) = chars.peek() {
                    if d.is_ascii_digit() {
                        num_str.push(d);
                        chars.next();
                    } else {
                        break;
                    }
                }
                let n: i64 = num_str.parse().map_err(|_| AutoError::Parse {
                    message: format!("Invalid number: {}", num_str),
                    line,
                })?;
                tokens.push(Located { token: Token::Number(n), line });
            }
            c if c.is_ascii_alphanumeric() || c == '_' => {
                let mut ident = String::new();
                while let Some(&c) = chars.peek() {
                    if c.is_ascii_alphanumeric() || c == '_' {
                        ident.push(c);
                        chars.next();
                    } else {
                        break;
                    }
                }
                let token = match ident.as_str() {
                    "foreach" => Token::Foreach,
                    "in" => Token::In,
                    "for" => Token::For,
                    "from" => Token::From,
                    "to" => Token::To,
                    "if" => Token::If,
                    "else" => Token::Else,
                    _ => Token::Ident(ident),
                };
                tokens.push(Located { token, line });
            }
            _ => {
                return Err(AutoError::Parse {
                    message: format!("Unexpected character: '{}'", ch),
                    line,
                });
            }
        }
    }

    Ok(tokens)
}

struct Parser {
    tokens: Vec<Located>,
    pos: usize,
}

impl Parser {
    fn new(tokens: Vec<Located>) -> Self {
        Self { tokens, pos: 0 }
    }

    fn current_line(&self) -> usize {
        self.tokens.get(self.pos).map_or(
            self.tokens.last().map_or(1, |t| t.line),
            |t| t.line,
        )
    }

    fn peek(&self) -> Option<&Token> {
        self.tokens.get(self.pos).map(|t| &t.token)
    }

    fn advance(&mut self) -> Option<&Token> {
        let t = self.tokens.get(self.pos).map(|t| &t.token);
        self.pos += 1;
        t
    }

    fn expect(&mut self, expected: &Token) -> Result<(), AutoError> {
        let line = self.current_line();
        match self.advance() {
            Some(t) if t == expected => Ok(()),
            Some(t) => Err(AutoError::Parse {
                message: format!("Expected {:?}, got {:?}", expected, t),
                line,
            }),
            None => Err(AutoError::Parse {
                message: format!("Expected {:?}, got end of input", expected),
                line,
            }),
        }
    }

    fn skip_newlines(&mut self) {
        while self.peek() == Some(&Token::Newline) {
            self.advance();
        }
    }

    fn parse_script(&mut self) -> Result<Script, AutoError> {
        let mut statements = Vec::new();
        self.skip_newlines();

        while self.pos < self.tokens.len() {
            self.skip_newlines();
            if self.pos >= self.tokens.len() {
                break;
            }
            statements.push(self.parse_statement()?);
            self.skip_newlines();
        }

        Ok(Script { statements })
    }

    fn parse_statement(&mut self) -> Result<Statement, AutoError> {
        match self.peek() {
            Some(Token::Foreach) => self.parse_foreach(),
            Some(Token::For) => self.parse_for(),
            Some(Token::If) => self.parse_if(),
            Some(Token::Ident(_)) => {
                // Look ahead: is this `ident = expr` or `ident(args)`?
                let ident_pos = self.pos;
                // Check if this is assignment: IDENT = expr  (but not IDENT == expr)
                if self.pos + 1 < self.tokens.len()
                    && self.tokens[self.pos + 1].token == Token::Equals
                {
                    self.parse_assignment()
                } else {
                    // Must be a command call (or bare ident as command with no args)
                    self.pos = ident_pos;
                    let call = self.parse_command_call()?;
                    Ok(Statement::Command(call))
                }
            }
            Some(other) => Err(AutoError::Parse {
                message: format!("Unexpected token: {:?}", other),
                line: self.current_line(),
            }),
            None => Err(AutoError::Parse {
                message: "Unexpected end of input".to_string(),
                line: self.current_line(),
            }),
        }
    }

    fn parse_assignment(&mut self) -> Result<Statement, AutoError> {
        let name = match self.advance() {
            Some(Token::Ident(s)) => s.clone(),
            _ => unreachable!(),
        };
        self.expect(&Token::Equals)?;
        let value = self.parse_expression()?;
        Ok(Statement::Assignment { variable: name, value })
    }

    fn parse_foreach(&mut self) -> Result<Statement, AutoError> {
        self.expect(&Token::Foreach)?;
        let variable = match self.advance() {
            Some(Token::Ident(s)) => s.clone(),
            _ => return Err(AutoError::Parse {
                message: "Expected variable name after 'foreach'".to_string(),
                line: self.current_line(),
            }),
        };
        self.expect(&Token::In)?;
        let collection = self.parse_expression()?;
        let body = self.parse_block()?;
        Ok(Statement::Foreach { variable, collection, body })
    }

    fn parse_for(&mut self) -> Result<Statement, AutoError> {
        self.expect(&Token::For)?;
        let variable = match self.advance() {
            Some(Token::Ident(s)) => s.clone(),
            _ => return Err(AutoError::Parse {
                message: "Expected variable name after 'for'".to_string(),
                line: self.current_line(),
            }),
        };
        self.expect(&Token::From)?;
        let from = match self.advance() {
            Some(Token::Number(n)) => *n,
            _ => return Err(AutoError::Parse {
                message: "Expected number after 'from'".to_string(),
                line: self.current_line(),
            }),
        };
        self.expect(&Token::To)?;
        let to = match self.advance() {
            Some(Token::Number(n)) => *n,
            _ => return Err(AutoError::Parse {
                message: "Expected number after 'to'".to_string(),
                line: self.current_line(),
            }),
        };
        let body = self.parse_block()?;
        Ok(Statement::For { variable, from, to, body })
    }

    fn parse_if(&mut self) -> Result<Statement, AutoError> {
        self.expect(&Token::If)?;
        let condition = self.parse_expression()?;
        let then_block = self.parse_block()?;

        self.skip_newlines();
        let else_block = if self.peek() == Some(&Token::Else) {
            self.advance();
            Some(self.parse_block()?)
        } else {
            None
        };

        Ok(Statement::If { condition, then_block, else_block })
    }

    fn parse_block(&mut self) -> Result<Vec<Statement>, AutoError> {
        self.skip_newlines();
        self.expect(&Token::LBrace)?;
        let mut stmts = Vec::new();
        self.skip_newlines();

        while self.peek() != Some(&Token::RBrace) {
            if self.pos >= self.tokens.len() {
                return Err(AutoError::Parse {
                    message: "Unclosed block, expected '}'".to_string(),
                    line: self.current_line(),
                });
            }
            stmts.push(self.parse_statement()?);
            self.skip_newlines();
        }

        self.expect(&Token::RBrace)?;
        Ok(stmts)
    }

    fn parse_command_call(&mut self) -> Result<CommandCall, AutoError> {
        let line = self.current_line();
        let name = match self.advance() {
            Some(Token::Ident(s)) => s.clone(),
            _ => return Err(AutoError::Parse {
                message: "Expected command name".to_string(),
                line,
            }),
        };

        // Commands without parens (bare commands like start_session, help)
        if self.peek() != Some(&Token::LParen) {
            return Ok(CommandCall { name, args: vec![], line });
        }

        self.expect(&Token::LParen)?;
        let mut args = Vec::new();

        if self.peek() != Some(&Token::RParen) {
            args.push(self.parse_expression()?);
            while self.peek() == Some(&Token::Comma) {
                self.advance();
                args.push(self.parse_expression()?);
            }
        }

        self.expect(&Token::RParen)?;
        Ok(CommandCall { name, args, line })
    }

    fn parse_expression(&mut self) -> Result<Expression, AutoError> {
        let left = self.parse_primary()?;

        match self.peek() {
            Some(Token::Plus) => {
                self.advance();
                let right = self.parse_expression()?;
                Ok(Expression::BinaryOp {
                    op: BinOp::Add,
                    left: Box::new(left),
                    right: Box::new(right),
                })
            }
            Some(Token::DoubleEquals) => {
                self.advance();
                let right = self.parse_expression()?;
                Ok(Expression::BinaryOp {
                    op: BinOp::Eq,
                    left: Box::new(left),
                    right: Box::new(right),
                })
            }
            Some(Token::NotEquals) => {
                self.advance();
                let right = self.parse_expression()?;
                Ok(Expression::BinaryOp {
                    op: BinOp::NotEq,
                    left: Box::new(left),
                    right: Box::new(right),
                })
            }
            _ => Ok(left),
        }
    }

    fn parse_primary(&mut self) -> Result<Expression, AutoError> {
        match self.peek() {
            Some(Token::String(_)) => {
                if let Some(Token::String(s)) = self.advance().cloned() {
                    Ok(Expression::String(s))
                } else {
                    unreachable!()
                }
            }
            Some(Token::InterpolatedString(_)) => {
                if let Some(Token::InterpolatedString(segments)) = self.advance().cloned() {
                    // Build a chain of BinaryOp::Add from segments
                    let mut exprs: Vec<Expression> = segments.into_iter().map(|seg| {
                        match seg {
                            StringSegment::Literal(s) => Expression::String(s),
                            StringSegment::Variable(name) => Expression::Variable(name),
                        }
                    }).collect();
                    // Fold left into Add chain
                    let first = exprs.remove(0);
                    Ok(exprs.into_iter().fold(first, |acc, expr| {
                        Expression::BinaryOp {
                            op: BinOp::Add,
                            left: Box::new(acc),
                            right: Box::new(expr),
                        }
                    }))
                } else {
                    unreachable!()
                }
            }
            Some(Token::Number(_)) => {
                if let Some(Token::Number(n)) = self.advance().cloned() {
                    Ok(Expression::Number(n))
                } else {
                    unreachable!()
                }
            }
            Some(Token::LBracket) => {
                self.advance();
                let mut items = Vec::new();
                if self.peek() != Some(&Token::RBracket) {
                    items.push(self.parse_expression()?);
                    while self.peek() == Some(&Token::Comma) {
                        self.advance();
                        items.push(self.parse_expression()?);
                    }
                }
                self.expect(&Token::RBracket)?;
                Ok(Expression::List(items))
            }
            Some(Token::Ident(_)) => {
                // Could be a variable or a command capture
                let ident_pos = self.pos;
                let name = match self.advance() {
                    Some(Token::Ident(s)) => s.clone(),
                    _ => unreachable!(),
                };

                if self.peek() == Some(&Token::LParen) {
                    // It's a command capture like get_value("selector")
                    self.pos = ident_pos;
                    let call = self.parse_command_call()?;
                    Ok(Expression::CommandCapture(call))
                } else {
                    Ok(Expression::Variable(name))
                }
            }
            Some(other) => Err(AutoError::Parse {
                message: format!("Unexpected token in expression: {:?}", other),
                line: self.current_line(),
            }),
            None => Err(AutoError::Parse {
                message: "Unexpected end of input in expression".to_string(),
                line: self.current_line(),
            }),
        }
    }
}

pub fn parse(source: &str) -> Result<Script, AutoError> {
    let tokens = tokenize(source)?;
    let mut parser = Parser::new(tokens);
    parser.parse_script()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_bare_command() {
        let script = parse("start_session").unwrap();
        assert_eq!(script.statements.len(), 1);
        match &script.statements[0] {
            Statement::Command(call) => {
                assert_eq!(call.name, "start_session");
                assert!(call.args.is_empty());
            }
            _ => panic!("Expected Command"),
        }
    }

    #[test]
    fn test_parse_command_with_string_arg() {
        let script = parse(r#"tap("login-button")"#).unwrap();
        assert_eq!(script.statements.len(), 1);
        match &script.statements[0] {
            Statement::Command(call) => {
                assert_eq!(call.name, "tap");
                assert_eq!(call.args.len(), 1);
                match &call.args[0] {
                    Expression::String(s) => assert_eq!(s, "login-button"),
                    _ => panic!("Expected String arg"),
                }
            }
            _ => panic!("Expected Command"),
        }
    }

    #[test]
    fn test_parse_command_with_multiple_args() {
        let script = parse(r#"wait_for("dashboard", 5000)"#).unwrap();
        match &script.statements[0] {
            Statement::Command(call) => {
                assert_eq!(call.name, "wait_for");
                assert_eq!(call.args.len(), 2);
                match &call.args[1] {
                    Expression::Number(n) => assert_eq!(*n, 5000),
                    _ => panic!("Expected Number arg"),
                }
            }
            _ => panic!("Expected Command"),
        }
    }

    #[test]
    fn test_parse_assignment() {
        let script = parse(r#"status = "Ready""#).unwrap();
        match &script.statements[0] {
            Statement::Assignment { variable, value } => {
                assert_eq!(variable, "status");
                match value {
                    Expression::String(s) => assert_eq!(s, "Ready"),
                    _ => panic!("Expected String value"),
                }
            }
            _ => panic!("Expected Assignment"),
        }
    }

    #[test]
    fn test_parse_assignment_with_command_capture() {
        let script = parse(r#"status = get_value("status-label")"#).unwrap();
        match &script.statements[0] {
            Statement::Assignment { variable, value } => {
                assert_eq!(variable, "status");
                match value {
                    Expression::CommandCapture(call) => {
                        assert_eq!(call.name, "get_value");
                        assert_eq!(call.args.len(), 1);
                    }
                    _ => panic!("Expected CommandCapture"),
                }
            }
            _ => panic!("Expected Assignment"),
        }
    }

    #[test]
    fn test_parse_if() {
        let script = parse(r#"
if status == "Ready" {
    tap("start-button")
}
"#).unwrap();
        match &script.statements[0] {
            Statement::If { condition, then_block, else_block } => {
                assert!(matches!(condition, Expression::BinaryOp { op: BinOp::Eq, .. }));
                assert_eq!(then_block.len(), 1);
                assert!(else_block.is_none());
            }
            _ => panic!("Expected If"),
        }
    }

    #[test]
    fn test_parse_if_else() {
        let script = parse(r#"
if status == "Ready" {
    tap("start-button")
} else {
    tap("refresh-button")
}
"#).unwrap();
        match &script.statements[0] {
            Statement::If { else_block, .. } => {
                assert!(else_block.is_some());
                assert_eq!(else_block.as_ref().unwrap().len(), 1);
            }
            _ => panic!("Expected If"),
        }
    }

    #[test]
    fn test_parse_foreach() {
        let script = parse(r#"
foreach account in accounts {
    tap("username-field")
    send_keys(account)
}
"#).unwrap();
        match &script.statements[0] {
            Statement::Foreach { variable, collection, body } => {
                assert_eq!(variable, "account");
                assert!(matches!(collection, Expression::Variable(v) if v == "accounts"));
                assert_eq!(body.len(), 2);
            }
            _ => panic!("Expected Foreach"),
        }
    }

    #[test]
    fn test_parse_foreach_with_list() {
        let script = parse(r#"
foreach item in ["a", "b", "c"] {
    send_keys(item)
}
"#).unwrap();
        match &script.statements[0] {
            Statement::Foreach { collection, .. } => {
                match collection {
                    Expression::List(items) => assert_eq!(items.len(), 3),
                    _ => panic!("Expected List"),
                }
            }
            _ => panic!("Expected Foreach"),
        }
    }

    #[test]
    fn test_parse_for() {
        let script = parse(r#"
for i from 1 to 5 {
    tap("step-" + i)
}
"#).unwrap();
        match &script.statements[0] {
            Statement::For { variable, from, to, body } => {
                assert_eq!(variable, "i");
                assert_eq!(*from, 1);
                assert_eq!(*to, 5);
                assert_eq!(body.len(), 1);
            }
            _ => panic!("Expected For"),
        }
    }

    #[test]
    fn test_parse_binary_ops() {
        let script = parse(r#"x = "hello" + " " + "world""#).unwrap();
        match &script.statements[0] {
            Statement::Assignment { value, .. } => {
                assert!(matches!(value, Expression::BinaryOp { op: BinOp::Add, .. }));
            }
            _ => panic!("Expected Assignment"),
        }
    }

    #[test]
    fn test_parse_not_equals() {
        let script = parse(r#"
if status != "Error" {
    tap("continue")
}
"#).unwrap();
        match &script.statements[0] {
            Statement::If { condition, .. } => {
                assert!(matches!(condition, Expression::BinaryOp { op: BinOp::NotEq, .. }));
            }
            _ => panic!("Expected If"),
        }
    }

    #[test]
    fn test_parse_comments() {
        let script = parse(r#"
# This is a comment
tap("button")
# Another comment
"#).unwrap();
        assert_eq!(script.statements.len(), 1);
    }

    #[test]
    fn test_parse_multiline_script() {
        let script = parse(r#"
start_session
tap("login-button")
send_keys("user@example.com")
wait_for("dashboard", 5000)
end_session
"#).unwrap();
        assert_eq!(script.statements.len(), 5);
    }

    #[test]
    fn test_parse_single_quotes() {
        let script = parse("tap('login-button')").unwrap();
        match &script.statements[0] {
            Statement::Command(call) => {
                match &call.args[0] {
                    Expression::String(s) => assert_eq!(s, "login-button"),
                    _ => panic!("Expected String"),
                }
            }
            _ => panic!("Expected Command"),
        }
    }

    #[test]
    fn test_parse_empty_list() {
        let script = parse("items = []").unwrap();
        match &script.statements[0] {
            Statement::Assignment { value, .. } => {
                match value {
                    Expression::List(items) => assert!(items.is_empty()),
                    _ => panic!("Expected List"),
                }
            }
            _ => panic!("Expected Assignment"),
        }
    }

    #[test]
    fn test_parse_error_unterminated_string() {
        let result = parse(r#"tap("unclosed)"#);
        assert!(result.is_err());
    }

    #[test]
    fn test_parse_error_unexpected_char() {
        let result = parse("tap(@)");
        assert!(result.is_err());
    }

    #[test]
    fn test_parse_command_no_args_with_parens() {
        let script = parse("get_screenshot()").unwrap();
        match &script.statements[0] {
            Statement::Command(call) => {
                assert_eq!(call.name, "get_screenshot");
                assert!(call.args.is_empty());
            }
            _ => panic!("Expected Command"),
        }
    }

    #[test]
    fn test_line_numbers_tracked() {
        let script = parse("start_session\ntap(\"btn\")\nend_session").unwrap();
        match &script.statements[1] {
            Statement::Command(call) => {
                assert_eq!(call.line, 2);
            }
            _ => panic!("Expected Command"),
        }
    }

    #[test]
    fn test_parse_string_escape_sequences() {
        let script = parse(r#"send_keys("line1\nline2")"#).unwrap();
        match &script.statements[0] {
            Statement::Command(call) => {
                match &call.args[0] {
                    Expression::String(s) => assert_eq!(s, "line1\nline2"),
                    _ => panic!("Expected String"),
                }
            }
            _ => panic!("Expected Command"),
        }
    }

    #[test]
    fn test_parse_interpolated_string_single_var() {
        let script = parse(r#"log("Hello $name")"#).unwrap();
        match &script.statements[0] {
            Statement::Command(call) => {
                assert_eq!(call.name, "log");
                assert_eq!(call.args.len(), 1);
                // Should be BinaryOp::Add("Hello ", Variable("name"))
                match &call.args[0] {
                    Expression::BinaryOp { op: BinOp::Add, left, right } => {
                        assert!(matches!(left.as_ref(), Expression::String(s) if s == "Hello "));
                        assert!(matches!(right.as_ref(), Expression::Variable(v) if v == "name"));
                    }
                    _ => panic!("Expected BinaryOp, got {:?}", call.args[0]),
                }
            }
            _ => panic!("Expected Command"),
        }
    }

    #[test]
    fn test_parse_interpolated_string_multiple_vars() {
        let script = parse(r#"log("Iteration:$i Screen: $sometext")"#).unwrap();
        match &script.statements[0] {
            Statement::Command(call) => {
                // Should produce: Add(Add(Add("Iteration:", var(i)), " Screen: "), var(sometext))
                assert!(matches!(&call.args[0], Expression::BinaryOp { op: BinOp::Add, .. }));
            }
            _ => panic!("Expected Command"),
        }
    }

    #[test]
    fn test_parse_interpolated_string_var_at_start() {
        let script = parse(r#"log("$name is here")"#).unwrap();
        match &script.statements[0] {
            Statement::Command(call) => {
                match &call.args[0] {
                    Expression::BinaryOp { op: BinOp::Add, left, right } => {
                        assert!(matches!(left.as_ref(), Expression::Variable(v) if v == "name"));
                        assert!(matches!(right.as_ref(), Expression::String(s) if s == " is here"));
                    }
                    _ => panic!("Expected BinaryOp, got {:?}", call.args[0]),
                }
            }
            _ => panic!("Expected Command"),
        }
    }

    #[test]
    fn test_parse_interpolated_string_var_at_end() {
        let script = parse(r#"log("Value: $val")"#).unwrap();
        match &script.statements[0] {
            Statement::Command(call) => {
                match &call.args[0] {
                    Expression::BinaryOp { op: BinOp::Add, left, right } => {
                        assert!(matches!(left.as_ref(), Expression::String(s) if s == "Value: "));
                        assert!(matches!(right.as_ref(), Expression::Variable(v) if v == "val"));
                    }
                    _ => panic!("Expected BinaryOp, got {:?}", call.args[0]),
                }
            }
            _ => panic!("Expected Command"),
        }
    }

    #[test]
    fn test_parse_single_quote_no_interpolation() {
        // Single quotes should NOT interpolate
        let script = parse("log('Hello $name')").unwrap();
        match &script.statements[0] {
            Statement::Command(call) => {
                match &call.args[0] {
                    Expression::String(s) => assert_eq!(s, "Hello $name"),
                    _ => panic!("Expected plain String (no interpolation in single quotes)"),
                }
            }
            _ => panic!("Expected Command"),
        }
    }

    #[test]
    fn test_parse_escaped_dollar_no_interpolation() {
        let script = parse(r#"log("Price: \$99")"#).unwrap();
        match &script.statements[0] {
            Statement::Command(call) => {
                match &call.args[0] {
                    Expression::String(s) => assert_eq!(s, "Price: $99"),
                    _ => panic!("Expected plain String (escaped dollar)"),
                }
            }
            _ => panic!("Expected Command"),
        }
    }

    #[test]
    fn test_parse_bare_dollar_no_interpolation() {
        // $ followed by non-identifier chars is treated as literal
        let script = parse(r#"log("Cost: $ 5")"#).unwrap();
        match &script.statements[0] {
            Statement::Command(call) => {
                match &call.args[0] {
                    Expression::String(s) => assert_eq!(s, "Cost: $ 5"),
                    _ => panic!("Expected plain String"),
                }
            }
            _ => panic!("Expected Command"),
        }
    }

    #[test]
    fn test_parse_interpolated_string_only_var() {
        let script = parse(r#"log("$x")"#).unwrap();
        match &script.statements[0] {
            Statement::Command(call) => {
                // Should be just Variable("x"), no wrapping needed
                assert!(matches!(&call.args[0], Expression::Variable(v) if v == "x"));
            }
            _ => panic!("Expected Command"),
        }
    }
}
