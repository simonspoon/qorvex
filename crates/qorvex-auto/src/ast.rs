#[derive(Debug, Clone)]
pub struct Script {
    pub statements: Vec<Statement>,
}

#[derive(Debug, Clone)]
pub enum Statement {
    Command(CommandCall),
    Assignment {
        variable: String,
        value: Expression,
    },
    Foreach {
        variable: String,
        collection: Expression,
        body: Vec<Statement>,
    },
    For {
        variable: String,
        from: i64,
        to: i64,
        body: Vec<Statement>,
    },
    If {
        condition: Expression,
        then_block: Vec<Statement>,
        else_block: Option<Vec<Statement>>,
    },
    Comment(String),
}

#[derive(Debug, Clone)]
pub struct CommandCall {
    pub name: String,
    pub args: Vec<Expression>,
    pub line: usize,
}

#[derive(Debug, Clone)]
pub enum Expression {
    String(String),
    Number(i64),
    Variable(String),
    List(Vec<Expression>),
    BinaryOp {
        op: BinOp,
        left: Box<Expression>,
        right: Box<Expression>,
    },
    CommandCapture(CommandCall),
}

#[derive(Debug, Clone, PartialEq)]
pub enum BinOp {
    Add,
    Eq,
    NotEq,
}
