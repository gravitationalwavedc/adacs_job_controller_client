venv/bin/python manage.py makemigrations
venv/bin/python manage.py migrate --database mysql

mysqldump -d -u jobclient -pjobclient jobclient > schema.ddl

venv/bin/python  ../../third_party/sqlpp11/scripts/ddl2cpp schema.ddl jobclient_schema schema

echo -e "// NOLINTBEGIN\n$(cat jobclient_schema.h)\n// NOLINTEND" > jobclient_schema.h
mv jobclient_schema.h ../
