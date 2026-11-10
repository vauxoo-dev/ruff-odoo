from odoo import models, tools
from odoo.tools import create_index
from odoo.tools import sql as odoo_sql


class IndexFunctionLiteral(models.Model):
    _name = "index.function.literal"

    def init(self):
        super().init()
        # ODW9507: every part of the call is a literal.
        tools.create_index(
            self.env.cr,
            "index_function_literal_ref_idx",
            self._table,
            ["ref"],
            where="ref IS NOT NULL",
        )
        # ODW9507: a literal table, keywords, an expression list and a method.
        tools.create_index(
            self._cr,
            indexname="index_function_literal_name_gin",
            tablename="index_function_literal",
            expressions=["name gin_trgm_ops"],
            method="gin",
        )
        # ODW9507: imported by name, a unique index.
        create_index(self.env.cr, "index_function_literal_code", self._table, ("code",), unique=True)
        # ODW9507: the deprecated spelling of a unique index.
        odoo_sql.create_unique_index(self.env.cr, "index_function_literal_uniq", self._table, ["a", "b"])


class IndexSql(models.Model):
    _name = "index.sql"

    def init(self):
        # ODW9507: plain SQL on the model's own table.
        self.env.cr.execute("CREATE INDEX IF NOT EXISTS index_sql_partner_idx ON index_sql (partner_id)")
        # ODW9507: unique, lower case, trailing semicolon.
        self._cr.execute("create unique index index_sql_code_uniq on index_sql (code);")

    def _auto_init(self):
        res = super()._auto_init()
        # ODW9507: `_auto_init` is reported as well.
        self.env.cr.execute(
            """
            CREATE INDEX index_sql_state_idx
                ON index_sql USING btree (state)
                WHERE state = 'draft'
            """
        )
        return res


class IndexInherit(models.Model):
    _inherit = "res.partner"

    def init(self):
        super().init()
        # ODW9507: the table comes from the single `_inherit`.
        self.env.cr.execute("CREATE INDEX res_partner_ref_idx ON res_partner (ref)")
        # ODW9507: `self._table` needs no table name.
        tools.create_index(self.env.cr, "res_partner_x_idx", self._table, ["x_ref"])


class IndexCustomTable(models.Model):
    _name = "index.custom.table"
    _table = "custom_table"

    def init(self):
        # ODW9507: `_table` wins over `_name`.
        self.env.cr.execute("CREATE INDEX custom_table_idx ON custom_table (a)")
        # Not reported: `_name` would give another table.
        self.env.cr.execute("CREATE INDEX index_custom_table_idx ON index_custom_table (a)")


class NotReportedDynamic(models.Model):
    _name = "not.reported.dynamic"

    def init(self):
        super().init()
        # One index per field: the loop cannot become attributes.
        for fname in ("a", "b"):
            tools.create_index(self.env.cr, f"not_reported_dynamic_{fname}", self._table, [fname])
        # Only when an extension is installed.
        if self.env.registry.has_trigram:
            tools.create_index(self.env.cr, "not_reported_dynamic_gin", self._table, ["name"])
        # Names and expressions held by variables.
        indexname = "not_reported_dynamic_var"
        tools.create_index(self.env.cr, indexname, self._table, ["a"])
        columns = ["a"]
        tools.create_index(self.env.cr, "not_reported_dynamic_cols", self._table, columns)
        # An f-string.
        tools.create_index(self.env.cr, f"{self._table}_fstring", self._table, ["a"])
        # A comment would be lost.
        tools.create_index(
            self.env.cr, "not_reported_dynamic_comment", self._table, ["a"], comment="c"
        )
        # A computed uniqueness.
        tools.create_index(self.env.cr, "not_reported_dynamic_uniq", self._table, ["a"], unique=flag)


class NotReportedSql(models.Model):
    _name = "not.reported.sql"

    def init(self):
        # CONCURRENTLY cannot run in the schema transaction.
        self.env.cr.execute("CREATE INDEX CONCURRENTLY idx_a ON not_reported_sql (a)")
        # Another table.
        self.env.cr.execute("CREATE INDEX idx_b ON res_partner (b)")
        # More than one statement.
        self.env.cr.execute("CREATE INDEX idx_c ON not_reported_sql (c); ANALYZE not_reported_sql")
        # Parameters and formatting.
        self.env.cr.execute("CREATE INDEX idx_d ON not_reported_sql (d) WHERE x = %s", ("a",))
        self.env.cr.execute("CREATE INDEX idx_e ON %s (e)" % self._table)
        self.env.cr.execute(f"CREATE INDEX idx_f ON {self._table} (f)")
        # Not an index.
        self.env.cr.execute("CREATE TABLE IF NOT EXISTS not_reported_sql_rel (a int)")
        self.env.cr.execute("DROP INDEX IF EXISTS idx_a")


class NotReportedNested(models.Model):
    _name = "not.reported.nested"

    def init(self):
        def helper():
            tools.create_index(self.env.cr, "not_reported_nested_idx", self._table, ["a"])

        helper()

    def other_method(self):
        # Only `init` and `_auto_init` are read.
        tools.create_index(self.env.cr, "not_reported_nested_other", self._table, ["a"])


class NotReportedView(models.Model):
    _name = "not.reported.view"
    _auto = False

    def init(self):
        tools.drop_view_if_exists(self.env.cr, self._table)
        self.env.cr.execute("CREATE VIEW not_reported_view AS (SELECT 1 AS id)")
        self.env.cr.execute("CREATE INDEX not_reported_view_idx ON not_reported_view (id)")
        tools.create_index(self.env.cr, "not_reported_view_idx2", self._table, ["id"])


class NotReportedMaterializedView(models.Model):
    _name = "not.reported.matview"

    def init(self):
        # No `_auto = False` here, the view in the body is enough.
        self.env.cr.execute("CREATE MATERIALIZED VIEW not_reported_matview AS (SELECT 1 AS id)")
        self.env.cr.execute("CREATE UNIQUE INDEX not_reported_matview_idx ON not_reported_matview (id)")


class NotReportedMixin(models.AbstractModel):
    _name = "not.reported.mixin"

    def init(self):
        tools.create_index(self.env.cr, "not_reported_mixin_idx", self._table, ["a"])


class NotReportedSeveralInherit(models.Model):
    _inherit = ["res.partner", "mail.thread"]

    def init(self):
        # The table is not known from the class.
        self.env.cr.execute("CREATE INDEX res_partner_idx ON res_partner (a)")
        # ODW9507: `self._table` is always the model's own.
        tools.create_index(self.env.cr, "res_partner_self_idx", self._table, ["a"])


class NotAModel:
    def init(self):
        tools.create_index(self.env.cr, "not_a_model_idx", "not_a_model", ["a"])


class AlreadyDeclared(models.Model):
    _name = "already.declared"

    _ref_idx = models.Index("(ref) WHERE ref IS NOT NULL")
    _code_uniq = models.UniqueIndex("(code)")

    def init(self):
        super().init()
