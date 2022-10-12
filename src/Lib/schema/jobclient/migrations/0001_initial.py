# Generated by Django 4.1 on 2022-10-03 05:13

from django.db import migrations, models
import django.db.models.deletion


class Migration(migrations.Migration):

    initial = True

    dependencies = [
    ]

    operations = [
        migrations.CreateModel(
            name='Job',
            fields=[
                ('id', models.BigAutoField(auto_created=True, primary_key=True, serialize=False, verbose_name='ID')),
                ('job_id', models.BigIntegerField(blank=True, db_index=True, default=None, null=True)),
                ('scheduler_id', models.BigIntegerField(blank=True, db_index=True, default=None, null=True)),
                ('submitting', models.BooleanField(default=False)),
                ('submitting_count', models.IntegerField(default=0)),
                ('bundle_hash', models.CharField(max_length=40)),
                ('working_directory', models.CharField(max_length=512)),
                ('queued', models.BooleanField(db_index=True, default=False)),
                ('params', models.TextField()),
                ('running', models.BooleanField(default=True)),
            ],
        ),
        migrations.CreateModel(
            name='JobStatus',
            fields=[
                ('id', models.BigAutoField(auto_created=True, primary_key=True, serialize=False, verbose_name='ID')),
                ('what', models.CharField(max_length=128)),
                ('state', models.IntegerField(db_index=True)),
                ('job', models.ForeignKey(on_delete=django.db.models.deletion.CASCADE, related_name='status', to='jobclient.job')),
            ],
        ),
    ]