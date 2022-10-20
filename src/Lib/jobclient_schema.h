// NOLINTBEGIN
// generated by ../../third_party/sqlpp11/scripts/ddl2cpp schema.ddl jobclient_schema schema
#ifndef SCHEMA_JOBCLIENT_SCHEMA_H
#define SCHEMA_JOBCLIENT_SCHEMA_H

#include <sqlpp11/table.h>
#include <sqlpp11/data_types.h>
#include <sqlpp11/char_sequence.h>

namespace schema
{
  namespace DjangoMigrations_
  {
    struct Id
    {
      struct _alias_t
      {
        static constexpr const char _literal[] =  "id";
        using _name_t = sqlpp::make_char_sequence<sizeof(_literal), _literal>;
        template<typename T>
        struct _member_t
          {
            T id;
            T& operator()() { return id; }
            const T& operator()() const { return id; }
          };
      };
      using _traits = sqlpp::make_traits<sqlpp::integer, sqlpp::tag::must_not_insert, sqlpp::tag::must_not_update>;
    };
    struct App
    {
      struct _alias_t
      {
        static constexpr const char _literal[] =  "app";
        using _name_t = sqlpp::make_char_sequence<sizeof(_literal), _literal>;
        template<typename T>
        struct _member_t
          {
            T app;
            T& operator()() { return app; }
            const T& operator()() const { return app; }
          };
      };
      using _traits = sqlpp::make_traits<sqlpp::text, sqlpp::tag::can_be_null>;
    };
    struct Name
    {
      struct _alias_t
      {
        static constexpr const char _literal[] =  "name";
        using _name_t = sqlpp::make_char_sequence<sizeof(_literal), _literal>;
        template<typename T>
        struct _member_t
          {
            T name;
            T& operator()() { return name; }
            const T& operator()() const { return name; }
          };
      };
      using _traits = sqlpp::make_traits<sqlpp::text, sqlpp::tag::can_be_null>;
    };
    struct Applied
    {
      struct _alias_t
      {
        static constexpr const char _literal[] =  "applied";
        using _name_t = sqlpp::make_char_sequence<sizeof(_literal), _literal>;
        template<typename T>
        struct _member_t
          {
            T applied;
            T& operator()() { return applied; }
            const T& operator()() const { return applied; }
          };
      };
      using _traits = sqlpp::make_traits<sqlpp::time_point, sqlpp::tag::require_insert>;
    };
  } // namespace DjangoMigrations_

  struct DjangoMigrations: sqlpp::table_t<DjangoMigrations,
               DjangoMigrations_::Id,
               DjangoMigrations_::App,
               DjangoMigrations_::Name,
               DjangoMigrations_::Applied>
  {
    struct _alias_t
    {
      static constexpr const char _literal[] =  "django_migrations";
      using _name_t = sqlpp::make_char_sequence<sizeof(_literal), _literal>;
      template<typename T>
      struct _member_t
      {
        T djangoMigrations;
        T& operator()() { return djangoMigrations; }
        const T& operator()() const { return djangoMigrations; }
      };
    };
  };
  namespace JobclientJob_
  {
    struct Id
    {
      struct _alias_t
      {
        static constexpr const char _literal[] =  "id";
        using _name_t = sqlpp::make_char_sequence<sizeof(_literal), _literal>;
        template<typename T>
        struct _member_t
          {
            T id;
            T& operator()() { return id; }
            const T& operator()() const { return id; }
          };
      };
      using _traits = sqlpp::make_traits<sqlpp::integer, sqlpp::tag::must_not_insert, sqlpp::tag::must_not_update>;
    };
    struct JobId
    {
      struct _alias_t
      {
        static constexpr const char _literal[] =  "job_id";
        using _name_t = sqlpp::make_char_sequence<sizeof(_literal), _literal>;
        template<typename T>
        struct _member_t
          {
            T jobId;
            T& operator()() { return jobId; }
            const T& operator()() const { return jobId; }
          };
      };
      using _traits = sqlpp::make_traits<sqlpp::integer, sqlpp::tag::can_be_null>;
    };
    struct SchedulerId
    {
      struct _alias_t
      {
        static constexpr const char _literal[] =  "scheduler_id";
        using _name_t = sqlpp::make_char_sequence<sizeof(_literal), _literal>;
        template<typename T>
        struct _member_t
          {
            T schedulerId;
            T& operator()() { return schedulerId; }
            const T& operator()() const { return schedulerId; }
          };
      };
      using _traits = sqlpp::make_traits<sqlpp::integer, sqlpp::tag::can_be_null>;
    };
    struct Submitting
    {
      struct _alias_t
      {
        static constexpr const char _literal[] =  "submitting";
        using _name_t = sqlpp::make_char_sequence<sizeof(_literal), _literal>;
        template<typename T>
        struct _member_t
          {
            T submitting;
            T& operator()() { return submitting; }
            const T& operator()() const { return submitting; }
          };
      };
      using _traits = sqlpp::make_traits<sqlpp::integer, sqlpp::tag::require_insert>;
    };
    struct SubmittingCount
    {
      struct _alias_t
      {
        static constexpr const char _literal[] =  "submitting_count";
        using _name_t = sqlpp::make_char_sequence<sizeof(_literal), _literal>;
        template<typename T>
        struct _member_t
          {
            T submittingCount;
            T& operator()() { return submittingCount; }
            const T& operator()() const { return submittingCount; }
          };
      };
      using _traits = sqlpp::make_traits<sqlpp::integer, sqlpp::tag::require_insert>;
    };
    struct BundleHash
    {
      struct _alias_t
      {
        static constexpr const char _literal[] =  "bundle_hash";
        using _name_t = sqlpp::make_char_sequence<sizeof(_literal), _literal>;
        template<typename T>
        struct _member_t
          {
            T bundleHash;
            T& operator()() { return bundleHash; }
            const T& operator()() const { return bundleHash; }
          };
      };
      using _traits = sqlpp::make_traits<sqlpp::text, sqlpp::tag::can_be_null>;
    };
    struct WorkingDirectory
    {
      struct _alias_t
      {
        static constexpr const char _literal[] =  "working_directory";
        using _name_t = sqlpp::make_char_sequence<sizeof(_literal), _literal>;
        template<typename T>
        struct _member_t
          {
            T workingDirectory;
            T& operator()() { return workingDirectory; }
            const T& operator()() const { return workingDirectory; }
          };
      };
      using _traits = sqlpp::make_traits<sqlpp::text, sqlpp::tag::can_be_null>;
    };
    struct Running
    {
      struct _alias_t
      {
        static constexpr const char _literal[] =  "running";
        using _name_t = sqlpp::make_char_sequence<sizeof(_literal), _literal>;
        template<typename T>
        struct _member_t
          {
            T running;
            T& operator()() { return running; }
            const T& operator()() const { return running; }
          };
      };
      using _traits = sqlpp::make_traits<sqlpp::integer, sqlpp::tag::require_insert>;
    };
    struct Deleted
    {
      struct _alias_t
      {
        static constexpr const char _literal[] =  "deleted";
        using _name_t = sqlpp::make_char_sequence<sizeof(_literal), _literal>;
        template<typename T>
        struct _member_t
          {
            T deleted;
            T& operator()() { return deleted; }
            const T& operator()() const { return deleted; }
          };
      };
      using _traits = sqlpp::make_traits<sqlpp::integer, sqlpp::tag::require_insert>;
    };
    struct Deleting
    {
      struct _alias_t
      {
        static constexpr const char _literal[] =  "deleting";
        using _name_t = sqlpp::make_char_sequence<sizeof(_literal), _literal>;
        template<typename T>
        struct _member_t
          {
            T deleting;
            T& operator()() { return deleting; }
            const T& operator()() const { return deleting; }
          };
      };
      using _traits = sqlpp::make_traits<sqlpp::integer, sqlpp::tag::require_insert>;
    };
  } // namespace JobclientJob_

  struct JobclientJob: sqlpp::table_t<JobclientJob,
               JobclientJob_::Id,
               JobclientJob_::JobId,
               JobclientJob_::SchedulerId,
               JobclientJob_::Submitting,
               JobclientJob_::SubmittingCount,
               JobclientJob_::BundleHash,
               JobclientJob_::WorkingDirectory,
               JobclientJob_::Running,
               JobclientJob_::Deleted,
               JobclientJob_::Deleting>
  {
    struct _alias_t
    {
      static constexpr const char _literal[] =  "jobclient_job";
      using _name_t = sqlpp::make_char_sequence<sizeof(_literal), _literal>;
      template<typename T>
      struct _member_t
      {
        T jobclientJob;
        T& operator()() { return jobclientJob; }
        const T& operator()() const { return jobclientJob; }
      };
    };
  };
  namespace JobclientJobstatus_
  {
    struct Id
    {
      struct _alias_t
      {
        static constexpr const char _literal[] =  "id";
        using _name_t = sqlpp::make_char_sequence<sizeof(_literal), _literal>;
        template<typename T>
        struct _member_t
          {
            T id;
            T& operator()() { return id; }
            const T& operator()() const { return id; }
          };
      };
      using _traits = sqlpp::make_traits<sqlpp::integer, sqlpp::tag::must_not_insert, sqlpp::tag::must_not_update>;
    };
    struct What
    {
      struct _alias_t
      {
        static constexpr const char _literal[] =  "what";
        using _name_t = sqlpp::make_char_sequence<sizeof(_literal), _literal>;
        template<typename T>
        struct _member_t
          {
            T what;
            T& operator()() { return what; }
            const T& operator()() const { return what; }
          };
      };
      using _traits = sqlpp::make_traits<sqlpp::text, sqlpp::tag::can_be_null>;
    };
    struct State
    {
      struct _alias_t
      {
        static constexpr const char _literal[] =  "state";
        using _name_t = sqlpp::make_char_sequence<sizeof(_literal), _literal>;
        template<typename T>
        struct _member_t
          {
            T state;
            T& operator()() { return state; }
            const T& operator()() const { return state; }
          };
      };
      using _traits = sqlpp::make_traits<sqlpp::integer, sqlpp::tag::require_insert>;
    };
    struct JobId
    {
      struct _alias_t
      {
        static constexpr const char _literal[] =  "job_id";
        using _name_t = sqlpp::make_char_sequence<sizeof(_literal), _literal>;
        template<typename T>
        struct _member_t
          {
            T jobId;
            T& operator()() { return jobId; }
            const T& operator()() const { return jobId; }
          };
      };
      using _traits = sqlpp::make_traits<sqlpp::integer, sqlpp::tag::require_insert>;
    };
  } // namespace JobclientJobstatus_

  struct JobclientJobstatus: sqlpp::table_t<JobclientJobstatus,
               JobclientJobstatus_::Id,
               JobclientJobstatus_::What,
               JobclientJobstatus_::State,
               JobclientJobstatus_::JobId>
  {
    struct _alias_t
    {
      static constexpr const char _literal[] =  "jobclient_jobstatus";
      using _name_t = sqlpp::make_char_sequence<sizeof(_literal), _literal>;
      template<typename T>
      struct _member_t
      {
        T jobclientJobstatus;
        T& operator()() { return jobclientJobstatus; }
        const T& operator()() const { return jobclientJobstatus; }
      };
    };
  };
} // namespace schema
#endif
// NOLINTEND
